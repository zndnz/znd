//! 提出前チェックと振り返り: 光過敏性の走査、版間差分、メイキング用の分解書き出し、ポイントトラッキング。

use crate::export::{ExportContext, ExportError};
use crate::MediaError;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use znd_core::{CompId, LayerKind, Project, Time};
use znd_render::check::{diff_frames, track_point, DiffReport, Gray};
use znd_render::safety::{analyze, frame_stats, FlashWarning};
use znd_render::{FootageSource, FrameBuf, RenderError, RenderOptions, Renderer};

fn renderer<'a>(project: &'a Project, ctx: &'a ExportContext<'a>) -> Renderer<'a> {
    let mut r = Renderer::new(project, ctx.footage).with_accel(ctx.accel).with_composite(ctx.composite);
    if let Some(res) = ctx.resolver {
        r = r.with_resolver(res);
    }
    if let Some(d) = &ctx.base_dir {
        r = r.with_base_dir(d.clone());
    }
    r
}

/// コンポ全尺を走査して、明滅・赤色フラッシュの危険がある区間を返す。
/// 解析は粗い格子で行うので、1/8 解像度でレンダリングして速度を稼ぐ。
pub fn scan_flashes(
    ctx: &ExportContext,
    comp_id: &CompId,
    mut progress: impl FnMut(i64, i64) -> bool,
) -> Result<Vec<FlashWarning>, ExportError> {
    let comp = ctx.project.comp(comp_id).ok_or_else(|| RenderError::MissingComp(comp_id.clone()))?;
    let r = renderer(ctx.project, ctx);
    let total = comp.frame_count();
    let opts = RenderOptions { downsample: 8, background: true, guides: false };
    let batch = rayon::current_num_threads().max(1) as i64 * 4;
    let mut stats = Vec::with_capacity(total as usize);
    let mut done = 0;
    while done < total {
        let frames: Vec<i64> = (done..(done + batch).min(total)).collect();
        let part: Vec<Result<_, RenderError>> =
            frames.par_iter().map(|f| Ok(frame_stats(&r.render_frame(comp_id, comp.frame_rate.frame_to_time(*f), opts)?))).collect();
        for s in part {
            stats.push(s?);
            done += 1;
        }
        if !progress(done, total) {
            return Err(ExportError::Cancelled);
        }
    }
    Ok(analyze(&stats, comp.frame_rate.fps()))
}

/// 2 つの版の、同じコンポ・同じ時刻の絵を比べる。
pub fn version_diff(
    a: &Project,
    b: &Project,
    footage: &dyn FootageSource,
    comp: &CompId,
    t: Time,
    downsample: u32,
) -> Result<DiffReport, ExportError> {
    let opts = RenderOptions { downsample, background: true, guides: false };
    let fa = Renderer::new(a, footage).render_frame(comp, t, opts)?;
    let fb = Renderer::new(b, footage).render_frame(comp, t, opts)?;
    diff_frames(&fa, &fb, 2.0 / 255.0).ok_or_else(|| MediaError::Invalid("2 つの版でコンポの解像度が違うため比較できません".into()).into())
}

/// 版間で絵が変わったフレーム番号を列挙する（`step` フレームおきに調べる）。
pub fn changed_frames(a: &Project, b: &Project, footage: &dyn FootageSource, comp: &CompId, step: i64) -> Result<Vec<i64>, ExportError> {
    let ca = a.comp(comp).ok_or_else(|| RenderError::MissingComp(comp.clone()))?;
    let frames: Vec<i64> = (0..ca.frame_count()).step_by(step.max(1) as usize).collect();
    let results: Vec<Result<Option<i64>, ExportError>> = frames
        .par_iter()
        .map(|f| {
            let d = version_diff(a, b, footage, comp, ca.frame_rate.frame_to_time(*f), 4)?;
            Ok((d.changed_pixels > 0).then_some(*f))
        })
        .collect();
    let mut out = Vec::new();
    for r in results {
        out.extend(r?);
    }
    Ok(out)
}

/// メイキング用の分解: 下のレイヤーから 1 枚ずつ重ねていった各段階の絵を返す。
/// 各要素は (その段階で加わったレイヤー名, 画像)。
pub fn breakdown(ctx: &ExportContext, comp_id: &CompId, t: Time, downsample: u32) -> Result<Vec<(String, FrameBuf)>, ExportError> {
    let comp = ctx.project.comp(comp_id).ok_or_else(|| RenderError::MissingComp(comp_id.clone()))?;
    let Some(ci) = ctx.project.compositions.iter().position(|c| &c.id == comp_id) else { return Ok(Vec::new()) };
    // 絵に関わるレイヤー（下から順）
    let stages: Vec<usize> = (0..comp.layers.len())
        .rev()
        .filter(|i| {
            let l = &comp.layers[*i];
            l.visible
                && !l.guide
                && l.active_at(t)
                && !matches!(l.kind, LayerKind::Null | LayerKind::Audio { .. } | LayerKind::Camera { .. } | LayerKind::Light { .. })
        })
        .collect();
    let opts = RenderOptions { downsample, background: true, guides: false };
    let results: Vec<Result<(String, FrameBuf), RenderError>> = stages
        .par_iter()
        .map(|&top| {
            // top より上（インデックスが小さい）の描画レイヤーを隠した版を作る
            let mut p = ctx.project.clone();
            for (i, l) in p.compositions[ci].layers.iter_mut().enumerate() {
                if i < top && stages.contains(&i) {
                    l.visible = false;
                }
            }
            let frame = renderer(&p, ctx).render_frame(comp_id, t, opts)?;
            Ok((comp.layers[top].name.clone(), frame))
        })
        .collect();
    let mut out = Vec::new();
    for r in results {
        out.push(r?);
    }
    Ok(out)
}

/// 分解を連番 PNG（`<prefix>_01_レイヤー名.png` …）として書き出す。
pub fn export_breakdown(ctx: &ExportContext, comp_id: &CompId, t: Time, dir: &Path, prefix: &str) -> Result<Vec<PathBuf>, ExportError> {
    std::fs::create_dir_all(dir).map_err(|e| MediaError::File { path: dir.display().to_string(), message: e.to_string() })?;
    let mut paths = Vec::new();
    for (i, (name, buf)) in breakdown(ctx, comp_id, t, 1)?.iter().enumerate() {
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        let path = dir.join(format!("{prefix}_{:02}_{safe}.png", i + 1));
        crate::export::save_png(buf, &path, None)?;
        paths.push(path);
    }
    Ok(paths)
}

/// コンポの絵の上で 1 点を追跡し、フレームごとの位置（コンポ座標）を返す。
/// 位置のキーフレームにすれば、他のレイヤーをその動きに追従させられる。
pub fn track(
    ctx: &ExportContext,
    comp_id: &CompId,
    start_frame: i64,
    end_frame: i64,
    point: [f64; 2],
    pattern: usize,
    search: usize,
) -> Result<Vec<(Time, [f64; 2])>, ExportError> {
    let comp = ctx.project.comp(comp_id).ok_or_else(|| RenderError::MissingComp(comp_id.clone()))?;
    let r = renderer(ctx.project, ctx);
    let frames: Vec<i64> = (start_frame..=end_frame.min(comp.frame_count() - 1)).collect();
    let grays: Vec<Result<Gray, RenderError>> = frames
        .par_iter()
        .map(|f| Ok(Gray::from_frame(&r.render_frame(comp_id, comp.frame_rate.frame_to_time(*f), RenderOptions::EXPORT)?)))
        .collect();
    let mut imgs = Vec::new();
    for g in grays {
        imgs.push(g?);
    }
    let positions = track_point(&imgs, point, pattern, search, 0.6);
    Ok(positions.into_iter().enumerate().map(|(i, p)| (comp.frame_rate.frame_to_time(frames[i]), p)).collect())
}
