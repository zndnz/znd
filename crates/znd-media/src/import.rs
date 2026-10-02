//! ファイルの種類を見分けて、コンポにレイヤーとして読み込む（アプリと CLI の共通の入口）。

use crate::{probe, psd, sequence, vector, MediaError};
use std::path::Path;
use znd_core::{camera_import, CompId, FootageId, FrameRate, Layer, LayerId, LayerKind, Project, Time, Transform};
use znd_render::alembic::Archive;

#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    /// 連番の 1 枚として扱う（同じ規則のファイルをまとめて 1 本の動画にする）。
    pub sequence: bool,
    /// 連番のフレームレート。
    pub sequence_rate: FrameRate,
    /// レイヤーを置く時刻。
    pub at: Time,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions { sequence: false, sequence_rate: sequence::DEFAULT_RATE, at: Time::ZERO }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ImportOutcome {
    /// 追加したレイヤー（`comp` の中）。
    pub layers: Vec<LayerId>,
    /// 新しく作ったコンポ（PSD）。
    pub new_comp: Option<CompId>,
    pub footage: Vec<FootageId>,
    pub warnings: Vec<String>,
    /// 何をどう読み込んだかの 1 行説明。
    pub summary: String,
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `path` をコンポ `comp_id` に読み込む。
pub fn import_file(project: &mut Project, comp_id: &CompId, path: &Path, opts: ImportOptions) -> Result<ImportOutcome, MediaError> {
    let comp = project.comp(comp_id).ok_or_else(|| MediaError::Invalid(format!("コンポ「{comp_id}」が見つかりません")))?;
    let (cw, ch, duration, fr) = (comp.width as f64, comp.height as f64, comp.duration, comp.frame_rate);
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out = ImportOutcome::default();
    let insert = |project: &mut Project, layer: Layer, out: &mut ImportOutcome| {
        out.layers.push(layer.id.clone());
        project.comp_mut(comp_id).expect("comp was just looked up").layers.insert(0, layer);
    };

    if vector::is_vector(path) {
        let v = vector::import_vector(path)?;
        out.summary = format!("{} をシェイプレイヤーとして読み込みました（パス {} 個、ベクターのまま）", file_name(path), v.path_count);
        out.warnings = v.warnings.clone();
        let id = project.fresh_id("shape");
        insert(project, v.into_layer(&id, cw, ch, duration), &mut out);
        return Ok(out);
    }
    if ext == "psd" || ext == "psb" {
        let before = project.footage.len();
        let (new_comp, warnings) = psd::import_psd(project, path, fr, duration)?;
        out.footage = project.footage[before..].iter().map(|f| f.id.clone()).collect();
        let (pw, ph, n) = project.comp(&new_comp).map(|c| (c.width as f64, c.height as f64, c.layers.len())).unwrap_or((cw, ch, 0));
        out.summary = format!("{} をレイヤー {} 枚のコンポとして読み込みました", file_name(path), n);
        out.warnings = warnings;
        let id = project.fresh_id("layer");
        let layer =
            Layer::new(id, file_name(path), LayerKind::Precomp { comp: new_comp.clone() }, Transform::centered(pw, ph, cw, ch), duration);
        insert(project, layer, &mut out);
        out.new_comp = Some(new_comp);
        return Ok(out);
    }

    let (fid, warnings) = if opts.sequence || sequence::is_pattern(path) {
        probe::import_sequence(project, path, opts.sequence_rate)?
    } else {
        probe::import_footage_with_warnings(project, path)?
    };
    out.warnings = warnings;
    out.footage.push(fid.clone());
    let item = project.footage_item(&fid).expect("footage was just added").clone();
    let id = project.fresh_id("layer");
    let layer = if probe::is_model(path) {
        let mut l = Layer::new(id, item.name.clone(), LayerKind::Model { footage: fid }, Transform::centered(0.0, 0.0, cw, ch), duration);
        l.three_d = true;
        out.summary = format!("{} を 3D モデルとして読み込みました", item.name);
        l
    } else {
        let (kind, tr) = if item.width == 0 {
            (LayerKind::Audio { footage: fid }, Transform::centered(0.0, 0.0, cw, ch))
        } else {
            (LayerKind::Footage { footage: fid }, Transform::centered(item.width as f64, item.height as f64, cw, ch))
        };
        let mut l = Layer::new(id, item.name.clone(), kind, tr, duration);
        // 素材レイヤーは指定時刻から始める
        let d = item.duration.or(item.audio.map(|a| a.duration));
        l.in_point = opts.at;
        l.start_time = opts.at;
        l.out_point = d.map(|d| (opts.at + d).min(duration)).unwrap_or(duration);
        out.summary = match (sequence::is_pattern(Path::new(&item.path)), item.frame_rate, item.duration) {
            (true, Some(r), Some(d)) => format!("{} を連番（{} コマ、{:.3}fps）として読み込みました", item.name, r.frame_count(d), r.fps()),
            _ => format!("{} を読み込みました", item.name),
        };
        l
    };
    insert(project, layer, &mut out);
    Ok(out)
}

/// Alembic のカメラを、コンポのフレームごとのキーフレームを持つカメラレイヤーにする。
/// `px_per_unit` は 3D ソフトの 1 単位を何 px とするか。
pub fn abc_camera_layer(path: &Path, comp_w: u32, comp_h: u32, fr: FrameRate, px_per_unit: f64) -> Result<Layer, MediaError> {
    let err = |message: String| MediaError::File { path: path.display().to_string(), message };
    let archive = Archive::open(path).map_err(MediaError::Invalid)?;
    let first = archive.scene_at(f64::MIN).map_err(err)?;
    if first.cameras.is_empty() {
        return Err(err("カメラが含まれていません".into()));
    }
    let frames = if first.animated { ((first.end - first.start) * fr.fps()).round() as i64 } else { 0 };
    // .chan と同じ並び（フレーム tx ty tz rx ry rz 垂直画角）に直して、既存の取り込み処理に渡す
    let mut chan = String::new();
    for f in 0..=frames {
        let s = archive.scene_at(first.start + f as f64 / fr.fps()).map_err(err)?;
        let cam = &s.cameras[0];
        let (p, fw, up) = (cam.position(), cam.forward(), cam.up());
        // 視線 = (-sin ry·cos rx, sin rx, -cos ry·cos rx)
        let rx = fw[1].clamp(-1.0, 1.0).asin();
        let ry = (-fw[0]).atan2(-fw[2]);
        // ロール: ロールなしの「上」と「右」に対する実際の上向きの角度
        let right0 = [ry.cos(), 0.0, -ry.sin()];
        let up0 =
            [right0[1] * -fw[2] - right0[2] * -fw[1], right0[2] * -fw[0] - right0[0] * -fw[2], right0[0] * -fw[1] - right0[1] * -fw[0]];
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let rz = (-dot(up, right0)).atan2(dot(up, up0));
        let v_aperture = cam.horizontal_aperture_mm * comp_h as f64 / comp_w as f64;
        let vfov = 2.0 * (v_aperture / (2.0 * cam.focal_length_mm.max(1e-6))).atan();
        chan.push_str(&format!(
            "{} {} {} {} {} {} {} {}\n",
            f,
            p[0],
            p[1],
            p[2],
            rx.to_degrees(),
            ry.to_degrees(),
            rz.to_degrees(),
            vfov.to_degrees()
        ));
    }
    let mut layer = camera_import::parse_chan(&chan, comp_w, comp_h, fr, px_per_unit, 39.6).map_err(err)?;
    layer.name = first.cameras[0].name.clone();
    Ok(layer)
}
