//! コンポジションの評価と合成。docs/03-evaluation-pipeline.md の実装。
//!
//! レイヤー 1 枚の流れ: ソース画像 → マスク → エフェクト → トランスフォーム → （モーションブラー）
//! → トラックマット → 描画モードで合成。シェイプ・テキスト・コラップスしたプリコンポは
//! 「連続ラスタライズ」で、トランスフォームを先に掛けて出力解像度で直接描く。

use crate::gpu_composite::{CompositeAccel, CompositeJob, CompositeLayer, CompositeSource};
use crate::blend::blend_tile;
use crate::color::{luma, ColorXf, OutputXf};
use crate::effects::{self, FxContext};
use crate::frame::{recycle, take_zeroed, FrameBuf, Tile};
use crate::mask::{apply_masks, combined_alpha};
use crate::raster::{rasterize, Source};
use crate::shape::{render_shape, transform_path};
use crate::text::{render_text, TextCtx};
use crate::three_d::{self, Scene, SceneItem};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use znd_core::eval::{layer_source_seconds, world_matrix};
use znd_core::shape::BezPath;
use znd_core::{Affine, BitDepth, CompId, Composition, FootageItem, Layer, LayerId, LayerKind, MaskMode, MatteMode, Project, Time};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("コンポ「{0}」が見つかりません")]
    MissingComp(CompId),
    #[error("レイヤー「{layer}」のフッテージが見つかりません")]
    MissingFootage { layer: String },
    #[error("フッテージ「{name}」を読み込めません: {message}")]
    Footage { name: String, message: String },
    #[error("プリコンポのネストが深すぎます（循環参照の可能性）")]
    TooDeep,
    #[error("エクスプレッションエラー: {0}")]
    Expression(String),
}

/// フッテージのフレームを供給する。レンダースレッドから並列に呼ばれる。
pub trait FootageSource: Sync {
    /// ソース時刻 `seconds` のフレーム（sRGB 原色のリニア・プリマルチプライド）。
    /// 尺の外など、表示すべきフレームが無いときは `Ok(None)`。
    /// 返す画像の解像度は `item` の公称サイズと異なってよい（プロキシ等）。
    fn frame(&self, item: &FootageItem, seconds: f64) -> Result<Option<Arc<FrameBuf>>, String>;
}

/// フッテージを使わないプロジェクト用。
pub struct NoFootage;

impl FootageSource for NoFootage {
    fn frame(&self, _: &FootageItem, _: f64) -> Result<Option<Arc<FrameBuf>>, String> {
        Err("フッテージソースが設定されていません".into())
    }
}

/// エクスプレッションの評価器（`znd-script` が実装する）。
pub trait Resolver: Sync {
    /// 時刻 `t` でエクスプレッションを評価し、その結果を定数として埋め込んだコンポを返す。
    /// エクスプレッションが 1 つも無ければ `Ok(None)`。
    fn resolve(&self, project: &Project, comp: &Composition, t: Time) -> Result<Option<Arc<Composition>>, String>;
    /// テキストのエクスプレッションセレクター。
    fn selector(&self, source: &str, text_index: usize, text_total: usize, selector_value: f64, t: Time) -> f64;
}

/// 音声の供給（オーディオスペクトラム等のエフェクト用。`znd-audio` が実装する）。
pub trait AudioWindow: Sync {
    /// レイヤーの音声（モノラル 48kHz）を、コンポ時刻 `center` を中心に `len` サンプル返す。
    fn window(&self, project: &Project, comp: &CompId, layer: &LayerId, center: Time, len: usize) -> Vec<f32>;
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct RenderOptions {
    /// 1 = フル、2 = 1/2、4 = 1/4 解像度。
    pub downsample: u32,
    /// コンポの背景色を敷く（false ならアルファ付きで返す）。
    pub background: bool,
    /// ガイドレイヤーを描く（プレビュー時 true、書き出し時 false）。
    pub guides: bool,
}

impl RenderOptions {
    pub const PREVIEW: RenderOptions = RenderOptions { downsample: 1, background: true, guides: true };
    pub const EXPORT: RenderOptions = RenderOptions { downsample: 1, background: true, guides: false };
}

const MAX_DEPTH: usize = 64;

/// 元のコンポ、またはエクスプレッション解決済みのコンポ。
enum CompRef<'c> {
    Borrowed(&'c Composition),
    Resolved(Arc<Composition>),
}

impl Deref for CompRef<'_> {
    type Target = Composition;
    fn deref(&self) -> &Composition {
        match self {
            CompRef::Borrowed(c) => c,
            CompRef::Resolved(a) => a,
        }
    }
}

/// レイヤーの「トランスフォーム前」の内容。
enum Content {
    Solid {
        width: f32,
        height: f32,
        color: [f32; 4],
    },
    /// 画像と、その画像が表すレイヤー座標上の大きさ・左上の位置（大きさを変えるエフェクトの後は 0 でない）。
    Image {
        buf: FrameBuf,
        width: f64,
        height: f64,
        x0: f64,
        y0: f64,
    },
    Shared {
        buf: Arc<FrameBuf>,
        width: f64,
        height: f64,
    },
}

pub struct Renderer<'a> {
    pub project: &'a Project,
    pub footage: &'a dyn FootageSource,
    pub resolver: Option<&'a dyn Resolver>,
    pub audio: Option<&'a dyn AudioWindow>,
    /// 相対パスの素材（3D モデル等）を探す基準ディレクトリ。
    pub base_dir: Option<PathBuf>,
    /// エフェクトの高速化バックエンド（GPU）。無ければすべて CPU で処理する。
    pub accel: Option<&'a dyn effects::EffectAccel>,
    /// 合成の高速化バックエンド（GPU）。無ければ CPU で合成する。
    pub composite: Option<&'a dyn CompositeAccel>,
    cx: ColorXf,
}

pub(crate) fn matrix_scale(m: &Affine) -> f32 {
    (m.a * m.d - m.b * m.c).abs().sqrt() as f32
}

/// 連続ラスタライズ（トランスフォームを先に掛けて出力解像度で描く）レイヤーか。
fn is_comp_space(layer: &Layer) -> bool {
    match &layer.kind {
        LayerKind::Shape { .. } | LayerKind::Text { .. } => true,
        // 3D のプリコンポは平面として扱う（コラップスは 2D のときだけ）
        LayerKind::Precomp { .. } => layer.collapse && !layer.three_d,
        _ => false,
    }
}

fn has_effects(layer: &Layer) -> bool {
    layer.effects.iter().any(|e| e.enabled)
}

fn has_masks(layer: &Layer) -> bool {
    layer.masks.iter().any(|m| m.mode != MaskMode::None)
}

impl<'a> Renderer<'a> {
    pub fn new(project: &'a Project, footage: &'a dyn FootageSource) -> Self {
        Renderer {
            project,
            footage,
            resolver: None,
            audio: None,
            base_dir: None,
            accel: None,
            composite: None,
            cx: ColorXf::new(project.settings.working_space),
        }
    }

    pub fn with_resolver(mut self, resolver: &'a dyn Resolver) -> Self {
        self.resolver = Some(resolver);
        self
    }

    pub fn with_audio(mut self, audio: &'a dyn AudioWindow) -> Self {
        self.audio = Some(audio);
        self
    }

    pub fn with_accel(mut self, accel: Option<&'a dyn effects::EffectAccel>) -> Self {
        self.accel = accel;
        self
    }

    pub fn with_composite(mut self, composite: Option<&'a dyn CompositeAccel>) -> Self {
        self.composite = composite;
        self
    }

    pub fn with_base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.base_dir = Some(dir.into());
        self
    }

    /// 作業色空間 → 出力色空間の変換（表示・書き出し用）。
    pub fn output_xf(&self) -> OutputXf {
        OutputXf::new(self.project.settings.working_space, self.project.settings.output_space)
    }

    /// 出力解像度（ダウンサンプル適用後）。
    pub fn output_size(comp: &Composition, downsample: u32) -> (usize, usize) {
        let ds = downsample.max(1);
        (comp.width.div_ceil(ds) as usize, comp.height.div_ceil(ds) as usize)
    }

    fn comp_at<'c>(&self, comp: &'c Composition, t: Time) -> Result<CompRef<'c>, RenderError> {
        match self.resolver {
            None => Ok(CompRef::Borrowed(comp)),
            Some(r) => match r.resolve(self.project, comp, t).map_err(RenderError::Expression)? {
                Some(resolved) => Ok(CompRef::Resolved(resolved)),
                None => Ok(CompRef::Borrowed(comp)),
            },
        }
    }

    pub fn render_frame(&self, comp_id: &CompId, t: Time, opts: RenderOptions) -> Result<FrameBuf, RenderError> {
        let comp = self.project.comp(comp_id).ok_or_else(|| RenderError::MissingComp(comp_id.clone()))?;
        let ds = opts.downsample.max(1) as f64;
        let (w, h) = Self::output_size(comp, opts.downsample);
        let mut canvas = self.render_layers(comp, t, Affine::scale(1.0 / ds, 1.0 / ds), w, h, opts, 0)?;
        if opts.background {
            let bg = self.cx.rgb(comp.background);
            for px in canvas.data.chunks_exact_mut(4) {
                let k = 1.0 - px[3];
                px[0] += bg[0] * k;
                px[1] += bg[1] * k;
                px[2] += bg[2] * k;
                px[3] = 1.0;
            }
        }
        // プラグインの出力フィルター（ガイドを描く＝プレビュー、描かない＝書き出し）
        let canvas = crate::plugins::hooks::apply_output_filters(comp, t, canvas, (1.0 / ds) as f32, &self.cx, opts.guides);
        Ok(canvas)
    }

    /// `orig` のレイヤーを下から順に合成する。`base` はコンポ座標 → 出力画素座標。
    #[allow(clippy::too_many_arguments)]
    fn render_layers(
        &self,
        orig: &Composition,
        t: Time,
        base: Affine,
        out_w: usize,
        out_h: usize,
        opts: RenderOptions,
        depth: usize,
    ) -> Result<FrameBuf, RenderError> {
        if depth > MAX_DEPTH {
            return Err(RenderError::TooDeep);
        }
        let comp_ref = self.comp_at(orig, t)?;
        let comp: &Composition = &comp_ref;
        let linear = self.project.settings.linear_blending;
        let clamp = self.project.settings.bit_depth != BitDepth::F32;
        let any_solo = comp.layers.iter().any(|l| l.solo);
        let drawable = |layer: &Layer| layer.visible && !(any_solo && !layer.solo) && !(layer.guide && !opts.guides) && layer.active_at(t);
        if let Some(gpu) = self.composite {
            // 3D のまとまりと調整レイヤーは「ここまでの絵」を CPU で読むので、そのコンポは CPU で合成する
            let cpu_only = comp.layers.iter().any(|l| {
                drawable(l)
                    && (three_d::is_scene_layer(l)
                        || matches!(l.kind, LayerKind::Adjustment)
                        || l.effects.iter().any(|e| e.enabled && znd_core::effect::reads_below(&e.kind)))
            });
            if !cpu_only && gpu.fits(out_w, out_h) {
                let job = self.composite_job(gpu, orig, comp, &drawable, t, base, out_w, out_h, opts, depth)?;
                match gpu.composite(&job) {
                    Ok(mut canvas) => {
                        if clamp {
                            quantize(&mut canvas, self.project.settings.bit_depth);
                        }
                        return Ok(canvas);
                    }
                    Err(e) => eprintln!("GPU の合成に失敗したため CPU で合成します: {e}"),
                }
            }
        }
        let mut canvas = FrameBuf { width: out_w, height: out_h, data: take_zeroed(out_w * out_h * 4) };

        let mut i = comp.layers.len();
        while i > 0 {
            i -= 1;
            let layer = &comp.layers[i];
            if three_d::is_scene_layer(layer) {
                // 連続する 3D レイヤーは 1 つのまとまりとして、カメラからの距離順に描く
                let mut run = Vec::new();
                if drawable(layer) {
                    run.push(i);
                }
                while i > 0 && !three_d::breaks_run(&comp.layers[i - 1]) {
                    i -= 1;
                    if three_d::is_scene_layer(&comp.layers[i]) && drawable(&comp.layers[i]) {
                        run.push(i);
                    }
                }
                if !run.is_empty() {
                    self.render_scene_run(&mut canvas, orig, comp, &run, t, base, opts, depth)?;
                }
                continue;
            }
            if !drawable(layer) {
                continue;
            }
            if matches!(layer.kind, LayerKind::Adjustment) {
                self.apply_adjustment(&mut canvas, orig, comp, layer, t, base, opts, depth)?;
                continue;
            }
            let Some(mut tile) = self.layer_tile(orig, comp, layer, t, base, out_w, out_h, opts, depth, Some(&canvas))? else { continue };
            if let Some(tm) = &layer.track_matte {
                // マットレイヤーは自身の表示スイッチに関係なくマットとして描く。
                let matte = match comp.layer(&tm.layer) {
                    Some(ml) if ml.active_at(t) => self.layer_tile(orig, comp, ml, t, base, out_w, out_h, opts, depth, None)?,
                    _ => None,
                };
                apply_matte(&mut tile, matte.as_ref(), tm.mode);
            }
            blend_tile(&mut canvas, &tile, layer.blend_mode, linear);
            if clamp {
                clamp_region(&mut canvas, &tile);
            }
        }
        if clamp {
            quantize(&mut canvas, self.project.settings.bit_depth);
        }
        Ok(canvas)
    }

    /// GPU で合成するための依頼を作る。GPU に載せられないレイヤーは CPU で出力空間のタイルまで作る。
    #[allow(clippy::too_many_arguments)]
    fn composite_job(
        &self,
        gpu: &dyn CompositeAccel,
        orig: &Composition,
        comp: &Composition,
        drawable: &dyn Fn(&Layer) -> bool,
        t: Time,
        base: Affine,
        out_w: usize,
        out_h: usize,
        opts: RenderOptions,
        depth: usize,
    ) -> Result<CompositeJob<'_>, RenderError> {
        let linear = self.project.settings.linear_blending;
        let clamp = self.project.settings.bit_depth != BitDepth::F32;
        let mut job = CompositeJob { width: out_w, height: out_h, linear, clamp, time_seconds: t.seconds(), cx: &self.cx, layers: Vec::new() };
        let mb = comp.motion_blur;
        for layer in comp.layers.iter().rev().filter(|l| drawable(l)) {
            let blur = mb.enabled && layer.motion_blur && mb.samples > 1 && mb.shutter_angle > 0.0;
            let simple = layer.track_matte.is_none() && !blur && !is_comp_space(layer) && !has_masks(layer);
            if simple {
                let opacity = (layer.transform.opacity.eval(t) / 100.0).clamp(0.0, 1.0) as f32;
                if opacity <= 0.0 {
                    continue;
                }
                let m = base.then_after(&world_matrix(comp, layer, t));
                let fx = has_effects(layer);
                let ops = || -> Vec<(String, znd_core::EvalParams)> {
                    layer.effects.iter().filter(|e| e.enabled).map(|e| (e.kind.clone(), e.eval(t))).collect()
                };
                let on_gpu = |ops: &[(String, znd_core::EvalParams)], scale: f32| {
                    let env = job.env(scale);
                    ops.iter().all(|(k, p)| gpu.supports_in_composite(&effects::AccelOp { kind: k, params: p }, &env))
                };
                if let (true, LayerKind::Solid { width, height, color }) = (fx, &layer.kind) {
                    // エフェクト付きの平面: CPU で塗った画像を作らず、GPU の上で塗る（`raw_content` と同じ大きさ）
                    let ds = opts.downsample.max(1);
                    let (bw, bh) = ((*width).div_ceil(ds).max(1) as usize, (*height).div_ceil(ds).max(1) as usize);
                    let (lw, lh) = (*width as f64, *height as f64);
                    let scale = matrix_scale(&Affine::scale(bw as f64 / lw, bh as f64 / lh));
                    let ops = ops();
                    if on_gpu(&ops, scale) {
                        let c = self.cx.rgb(*color);
                        let fit = Affine::scale(lw / bw as f64, lh / bh as f64);
                        job.layers.push(CompositeLayer {
                            source: CompositeSource::Fill { width: bw, height: bh, color: [c[0], c[1], c[2], 1.0], ops, effect_scale: scale },
                            matrix: m.then_after(&fit),
                            opacity,
                            blend: layer.blend_mode,
                        });
                        continue;
                    }
                }
                let content = self.raw_content(comp, layer, t, opts, depth, fx)?;
                let source = match content {
                    None => continue,
                    Some(Content::Solid { width, height, color }) => {
                        Some((CompositeSource::Solid { width: width as f64, height: height as f64, color }, m))
                    }
                    Some(Content::Image { buf, width, height, x0, y0 }) => {
                        let fit = Affine::translate(x0, y0).then_after(&Affine::scale(width / buf.width as f64, height / buf.height as f64));
                        let to_buf = Affine::scale(buf.width as f64 / width, buf.height as f64 / height);
                        let scale = matrix_scale(&to_buf);
                        let ops = ops();
                        on_gpu(&ops, scale).then(|| (CompositeSource::Image { buf: Arc::new(buf), ops, effect_scale: scale }, m.then_after(&fit)))
                    }
                    Some(Content::Shared { buf, width, height }) => {
                        let fit = Affine::scale(width / buf.width as f64, height / buf.height as f64);
                        Some((CompositeSource::Image { buf, ops: Vec::new(), effect_scale: 1.0 }, m.then_after(&fit)))
                    }
                };
                if let Some((source, matrix)) = source {
                    job.layers.push(CompositeLayer { source, matrix, opacity, blend: layer.blend_mode });
                    continue;
                }
            }
            // GPU に載せられない: CPU でタイルまで作る（マットもここで適用する）
            let Some(mut tile) = self.layer_tile(orig, comp, layer, t, base, out_w, out_h, opts, depth, None)? else { continue };
            if let Some(tm) = &layer.track_matte {
                let matte = match comp.layer(&tm.layer) {
                    Some(ml) if ml.active_at(t) => self.layer_tile(orig, comp, ml, t, base, out_w, out_h, opts, depth, None)?,
                    _ => None,
                };
                apply_matte(&mut tile, matte.as_ref(), tm.mode);
            }
            job.layers.push(CompositeLayer { source: CompositeSource::Tile(tile), matrix: Affine::IDENTITY, opacity: 1.0, blend: layer.blend_mode });
        }
        Ok(job)
    }

    /// 3D レイヤーのまとまりを描く（投影・ライティング・深度合成は `three_d.rs`）。
    #[allow(clippy::too_many_arguments)]
    fn render_scene_run(
        &self,
        canvas: &mut FrameBuf,
        orig: &Composition,
        comp: &Composition,
        run: &[usize],
        t: Time,
        base: Affine,
        opts: RenderOptions,
        depth: usize,
    ) -> Result<(), RenderError> {
        let scene = Scene::new(comp, t, base, canvas.width, canvas.height, &self.cx);
        let mut items = Vec::new();
        for &i in run {
            let layer = &comp.layers[i];
            let opacity = (layer.transform.opacity.eval(t) / 100.0).clamp(0.0, 1.0) as f32;
            if opacity <= 0.0 {
                continue;
            }
            if let LayerKind::Model { footage } = &layer.kind {
                let item = self.project.footage_item(footage).ok_or_else(|| RenderError::MissingFootage { layer: layer.name.clone() })?;
                items.push(SceneItem::Model { layer, path: item.path.clone(), opacity });
                continue;
            }
            // 平面の絵は 2D と同じ手順（マスク・エフェクト込み）でレイヤー座標に描いておく
            let (content, origin) = if is_comp_space(layer) {
                // シェイプ・テキストは寸法を持たないので、レイヤー原点を中心にコンポと同じ大きさの窓で描く
                let (cw, ch) = Self::output_size(comp, opts.downsample);
                let flat = Affine::scale(cw as f64 / comp.width as f64, ch as f64 / comp.height as f64);
                let origin = [-(comp.width as f64) / 2.0, -(comp.height as f64) / 2.0];
                let m = flat.then_after(&Affine::translate(-origin[0], -origin[1]));
                let buf = self.comp_space_buffer(orig, comp, layer, t, m, flat, cw, ch, opts, depth, None)?;
                (Some(Content::Image { buf, width: comp.width as f64, height: comp.height as f64, x0: 0.0, y0: 0.0 }), origin)
            } else {
                (self.layer_content(orig, comp, layer, t, opts, depth, None)?, [0.0, 0.0])
            };
            let Some(content) = content else { continue };
            let (buf, width, height) = match content {
                Content::Solid { width, height, color } => {
                    let mut b = FrameBuf::new(2, 2);
                    b.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&color));
                    (Arc::new(b), width as f64, height as f64)
                }
                Content::Image { buf, width, height, x0, y0 } => {
                    let origin = [origin[0] + x0, origin[1] + y0];
                    items.push(SceneItem::Plane { layer, buf: Arc::new(buf), width, height, opacity, origin });
                    continue;
                }
                Content::Shared { buf, width, height } => (buf, width, height),
            };
            items.push(SceneItem::Plane { layer, buf, width, height, opacity, origin });
        }
        scene.render(canvas, comp, &items, t, self.base_dir.as_deref());
        Ok(())
    }

    /// 調整レイヤー: ここまでの合成結果にエフェクトを掛け、マスクと不透明度の範囲でだけ置き換える。
    #[allow(clippy::too_many_arguments)]
    fn apply_adjustment(
        &self,
        canvas: &mut FrameBuf,
        orig: &Composition,
        comp: &Composition,
        layer: &Layer,
        t: Time,
        base: Affine,
        opts: RenderOptions,
        depth: usize,
    ) -> Result<(), RenderError> {
        let opacity = (layer.transform.opacity.eval(t) / 100.0).clamp(0.0, 1.0) as f32;
        if opacity <= 0.0 || !has_effects(layer) {
            return Ok(());
        }
        let m = base.then_after(&world_matrix(comp, layer, t));
        // 別時刻の「下の絵」は持っていないので、時間系エフェクトは現在の絵だけを見る
        let none = |_: Time| None;
        let below = (&*canvas, Affine::IDENTITY);
        let (processed, offset) = self.apply_effects(orig, comp, layer, t, canvas.clone(), matrix_scale(&base), &m, &none, opts, depth, Some(below))?;
        let processed = if processed.width != canvas.width || processed.height != canvas.height || offset != [0.0, 0.0] {
            paste_at(&processed, offset, canvas.width, canvas.height)
        } else {
            processed
        };
        let alpha = combined_alpha(&layer.masks, t, &m, canvas.width, canvas.height);
        for (i, (dst, src)) in canvas.data.chunks_exact_mut(4).zip(processed.data.chunks_exact(4)).enumerate() {
            let a = opacity * alpha.as_ref().map(|m| m[i]).unwrap_or(1.0);
            for k in 0..4 {
                dst[k] += (src[k] - dst[k]) * a;
            }
        }
        recycle(processed.data);
        Ok(())
    }

    /// レイヤー 1 枚を出力空間のタイルにする（不透明度・マスク・エフェクト・モーションブラー適用済み）。
    #[allow(clippy::too_many_arguments)]
    fn layer_tile(
        &self,
        orig: &Composition,
        comp: &Composition,
        layer: &Layer,
        t: Time,
        base: Affine,
        out_w: usize,
        out_h: usize,
        opts: RenderOptions,
        depth: usize,
        below: Option<&FrameBuf>,
    ) -> Result<Option<Tile>, RenderError> {
        let mb = comp.motion_blur;
        let blur = mb.enabled && layer.motion_blur && mb.samples > 1 && mb.shutter_angle > 0.0;
        let comp_space = is_comp_space(layer);
        // 画像系のレイヤーは、内容（ソース＋マスク＋エフェクト）を 1 回だけ作り、置き方だけを時刻ごとに変える
        // 下のレイヤーまでの絵は、レイヤー座標 → 出力画素の変換と一緒に渡す
        let below_at = below.map(|b| (b, base.then_after(&world_matrix(comp, layer, t))));
        let content = if comp_space { None } else { self.layer_content(orig, comp, layer, t, opts, depth, below_at)? };
        if !comp_space && content.is_none() {
            return Ok(None);
        }

        let place = |c: &Composition, l: &Layer, at: Time| -> Result<Option<Tile>, RenderError> {
            let opacity = (l.transform.opacity.eval(at) / 100.0).clamp(0.0, 1.0) as f32;
            if opacity <= 0.0 {
                return Ok(None);
            }
            let m = base.then_after(&world_matrix(c, l, at));
            match &content {
                None => {
                    let mut buf = self.comp_space_buffer(orig, c, l, at, m, base, out_w, out_h, opts, depth, if at == t { below } else { None })?;
                    if opacity < 1.0 {
                        buf.data.iter_mut().for_each(|v| *v *= opacity);
                    }
                    Ok(Some(Tile { x0: 0, y0: 0, w: out_w, h: out_h, data: buf.data }))
                }
                Some(Content::Solid { width, height, color }) => {
                    Ok(rasterize(&Source::Solid { width: *width, height: *height, color: *color }, &m, out_w, out_h, opacity))
                }
                Some(Content::Image { buf, width, height, x0, y0 }) => {
                    let fit = Affine::translate(*x0, *y0).then_after(&Affine::scale(width / buf.width as f64, height / buf.height as f64));
                    Ok(rasterize(&Source::Image(buf), &m.then_after(&fit), out_w, out_h, opacity))
                }
                Some(Content::Shared { buf, width, height }) => {
                    let fit = Affine::scale(width / buf.width as f64, height / buf.height as f64);
                    Ok(rasterize(&Source::Image(buf), &m.then_after(&fit), out_w, out_h, opacity))
                }
            }
        };

        if !blur {
            return place(comp, layer, t);
        }

        // モーションブラー: シャッターが開いている間の複数時刻で置き直して平均する
        let frame = comp.frame_rate.frame_to_time(1).0 as f64;
        // プレビュー解像度を落としているときはサンプル数も減らす
        let n = (mb.samples / opts.downsample.max(1)).max(4) as usize;
        let mut acc = take_zeroed(out_w * out_h * 4);
        let mut any = false;
        for i in 0..n {
            let phase = (mb.shutter_phase + mb.shutter_angle * (i as f64 + 0.5) / n as f64) / 360.0;
            let at = t + Time((phase * frame).round() as i64);
            let c = self.comp_at(orig, at)?;
            let Some(l) = c.layer(&layer.id) else { continue };
            let Some(tile) = place(&c, l, at)? else { continue };
            any = true;
            for row in 0..tile.h {
                let dst = ((tile.y0 + row) * out_w + tile.x0) * 4;
                let src = &tile.data[row * tile.w * 4..(row + 1) * tile.w * 4];
                acc[dst..dst + tile.w * 4].iter_mut().zip(src).for_each(|(a, s)| *a += s);
            }
        }
        if !any {
            recycle(acc);
            return Ok(None);
        }
        let k = 1.0 / n as f32;
        acc.iter_mut().for_each(|v| *v *= k);
        Ok(Some(Tile { x0: 0, y0: 0, w: out_w, h: out_h, data: acc }))
    }

    /// 画像系レイヤー（平面・フッテージ・プリコンポ）の内容。マスクとエフェクトを適用済み。
    fn layer_content(
        &self,
        orig: &Composition,
        comp: &Composition,
        layer: &Layer,
        t: Time,
        opts: RenderOptions,
        depth: usize,
        below: Option<(&FrameBuf, Affine)>,
    ) -> Result<Option<Content>, RenderError> {
        let processing = has_effects(layer) || has_masks(layer);
        let Some(content) = self.raw_content(comp, layer, t, opts, depth, processing)? else { return Ok(None) };
        if !processing {
            return Ok(Some(content));
        }
        let Content::Image { mut buf, width, height, .. } = content else { return Ok(Some(content)) };
        let to_buf = Affine::scale(buf.width as f64 / width, buf.height as f64 / height);
        apply_masks(&mut buf, &layer.masks, t, &to_buf);
        // 時間系エフェクト用: 別時刻の「マスクまで適用した絵」
        let at_time = |at: Time| -> Option<FrameBuf> {
            let c = self.comp_at(orig, at).ok()?;
            let l = c.layer(&layer.id)?;
            match self.raw_content(&c, l, at, opts, depth, true).ok()?? {
                Content::Image { mut buf, .. } => {
                    apply_masks(&mut buf, &l.masks, at, &to_buf);
                    Some(buf)
                }
                _ => None,
            }
        };
        let (sx, sy) = (buf.width as f64 / width, buf.height as f64 / height);
        // バッファ画素 → 出力画素
        let below = below.and_then(|(b, m)| to_buf.inverse().map(|inv| (b, m.then_after(&inv))));
        let (buf, [ox, oy]) = self.apply_effects(orig, comp, layer, t, buf, matrix_scale(&to_buf), &to_buf, &at_time, opts, depth, below)?;
        // 大きさを変えるエフェクトの後は、画像の大きさと左上の位置をレイヤー座標に直す
        let (width, height) = (buf.width as f64 / sx, buf.height as f64 / sy);
        Ok(Some(Content::Image { buf, width, height, x0: ox / sx, y0: oy / sy }))
    }

    /// マスク・エフェクト適用前の内容。`need_buffer` が true なら必ず書き換え可能な画像で返す。
    fn raw_content(
        &self,
        comp: &Composition,
        layer: &Layer,
        t: Time,
        opts: RenderOptions,
        depth: usize,
        need_buffer: bool,
    ) -> Result<Option<Content>, RenderError> {
        let ds = opts.downsample.max(1);
        let seconds = layer_source_seconds(comp, layer, t);
        match &layer.kind {
            LayerKind::Solid { width, height, color } => {
                let c = self.cx.rgb(*color);
                let color = [c[0], c[1], c[2], 1.0];
                if !need_buffer {
                    return Ok(Some(Content::Solid { width: *width as f32, height: *height as f32, color }));
                }
                let (bw, bh) = ((*width).div_ceil(ds).max(1) as usize, (*height).div_ceil(ds).max(1) as usize);
                let mut buf = FrameBuf::new(bw, bh);
                buf.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&color));
                Ok(Some(Content::Image { buf, width: *width as f64, height: *height as f64, x0: 0.0, y0: 0.0 }))
            }
            LayerKind::Footage { footage } => {
                let item = self.project.footage_item(footage).ok_or_else(|| RenderError::MissingFootage { layer: layer.name.clone() })?;
                if item.width == 0 || item.height == 0 {
                    return Ok(None);
                }
                let fetch =
                    |s: f64| self.footage.frame(item, s).map_err(|message| RenderError::Footage { name: item.name.clone(), message });
                let Some(frame) = fetch(seconds)? else { return Ok(None) };
                let (width, height) = (item.width as f64, item.height as f64);
                // フレームブレンド: 前後のフレームを時間の端数で混ぜる
                let blended = match (layer.frame_blend, item.frame_rate) {
                    (true, Some(fr)) => {
                        let pos = seconds * fr.fps();
                        let frac = (pos - pos.floor()) as f32;
                        match fetch((pos.floor() + 1.0) / fr.fps() + 1e-6)? {
                            Some(next) if frac > 1e-3 && next.width == frame.width && next.height == frame.height => {
                                let mut mixed = (*frame).clone();
                                mixed.data.iter_mut().zip(&next.data).for_each(|(a, b)| *a += (*b - *a) * frac);
                                Some(mixed)
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if !need_buffer && self.cx.identity {
                    return Ok(Some(match blended {
                        Some(buf) => Content::Image { buf, width, height, x0: 0.0, y0: 0.0 },
                        None => Content::Shared { buf: frame, width, height },
                    }));
                }
                // エフェクトを掛ける場合は、プレビュー解像度まで落とした書き換え可能なバッファにする
                let src = blended.as_ref().unwrap_or(&frame);
                let (bw, bh) = if need_buffer {
                    (item.width.div_ceil(ds).max(1) as usize, item.height.div_ceil(ds).max(1) as usize)
                } else {
                    (src.width, src.height)
                };
                let mut buf = if bw == src.width && bh == src.height {
                    src.clone()
                } else {
                    let fit = Affine::scale(bw as f64 / src.width as f64, bh as f64 / src.height as f64);
                    tile_to_buf(rasterize(&Source::Image(src), &fit, bw, bh, 1.0), bw, bh)
                };
                self.cx.convert_buffer(&mut buf.data);
                Ok(Some(Content::Image { buf, width, height, x0: 0.0, y0: 0.0 }))
            }
            LayerKind::Precomp { comp: nested_id } if !layer.collapse || layer.three_d => {
                let nested = self.project.comp(nested_id).ok_or_else(|| RenderError::MissingComp(nested_id.clone()))?;
                let local = Time::from_seconds(seconds);
                if local < Time::ZERO || local >= nested.duration {
                    return Ok(None);
                }
                // ネストしたコンポは、そのコンポ自身のフレームグリッドで評価する。
                let fr = nested.frame_rate;
                let nested_t = fr.frame_to_time(fr.time_to_frame(local));
                let (nw, nh) = Self::output_size(nested, opts.downsample);
                let s = 1.0 / ds as f64;
                let buf = self.render_layers(nested, nested_t, Affine::scale(s, s), nw, nh, opts, depth + 1)?;
                Ok(Some(Content::Image { buf, width: nested.width as f64, height: nested.height as f64, x0: 0.0, y0: 0.0 }))
            }
            _ => Ok(None),
        }
    }

    /// 連続ラスタライズのレイヤーを、出力解像度のバッファに描く（マスク・エフェクト適用済み、不透明度は未適用）。
    /// `m` はレイヤー座標 → 出力画素、`base` はコンポ座標 → 出力画素。
    #[allow(clippy::too_many_arguments)]
    fn comp_space_buffer(
        &self,
        orig: &Composition,
        comp: &Composition,
        layer: &Layer,
        t: Time,
        m: Affine,
        base: Affine,
        out_w: usize,
        out_h: usize,
        opts: RenderOptions,
        depth: usize,
        below: Option<&FrameBuf>,
    ) -> Result<FrameBuf, RenderError> {
        let mut buf = self.comp_space_raw(comp, layer, t, m, out_w, out_h, opts, depth)?;
        apply_masks(&mut buf, &layer.masks, t, &m);
        if !has_effects(layer) {
            return Ok(buf);
        }
        let at_time = |at: Time| -> Option<FrameBuf> {
            let c = self.comp_at(orig, at).ok()?;
            let l = c.layer(&layer.id)?;
            let m2 = base.then_after(&world_matrix(&c, l, at));
            let mut b = self.comp_space_raw(&c, l, at, m2, out_w, out_h, opts, depth).ok()?;
            apply_masks(&mut b, &l.masks, at, &m2);
            Some(b)
        };
        let below = below.map(|b| (b, Affine::IDENTITY));
        let (buf, offset) = self.apply_effects(orig, comp, layer, t, buf, matrix_scale(&base), &m, &at_time, opts, depth, below)?;
        Ok(if buf.width != out_w || buf.height != out_h || offset != [0.0, 0.0] { paste_at(&buf, offset, out_w, out_h) } else { buf })
    }

    #[allow(clippy::too_many_arguments)]
    fn comp_space_raw(
        &self,
        comp: &Composition,
        layer: &Layer,
        t: Time,
        m: Affine,
        out_w: usize,
        out_h: usize,
        opts: RenderOptions,
        depth: usize,
    ) -> Result<FrameBuf, RenderError> {
        let mut buf = FrameBuf { width: out_w, height: out_h, data: take_zeroed(out_w * out_h * 4) };
        match &layer.kind {
            LayerKind::Shape { items } => render_shape(items, t, &m, &mut buf, &self.cx),
            LayerKind::Text { document, animators, path, per_char_3d, .. } => {
                let path_geo: Option<BezPath> = path.as_ref().and_then(|p| layer.masks.get(p.mask)).map(|k| k.path.eval(t));
                let selector = |src: &str, index: usize, total: usize, value: f64| match self.resolver {
                    Some(r) => r.selector(src, index, total, value, t),
                    None => value,
                };
                // 文字ごとの 3D の遠近は、既定カメラ（50mm 相当）の距離を基準にする
                let ctx = TextCtx { cx: &self.cx, selector_expr: &selector, zoom: three_d::default_zoom(comp) };
                render_text(document, animators, path.as_ref().zip(path_geo.as_ref()), *per_char_3d, t, &m, &mut buf, &ctx);
            }
            LayerKind::Precomp { comp: nested_id } => {
                let nested = self.project.comp(nested_id).ok_or_else(|| RenderError::MissingComp(nested_id.clone()))?;
                let local = Time::from_seconds(layer_source_seconds(comp, layer, t));
                if local >= Time::ZERO && local < nested.duration {
                    let fr = nested.frame_rate;
                    let nested_t = fr.frame_to_time(fr.time_to_frame(local));
                    // コラップス: 中のレイヤーを親の出力空間へ直接ラスタライズする
                    // （プリコンポ境界で切り取られず、再サンプリングも 1 回で済む）。
                    recycle(std::mem::take(&mut buf.data));
                    buf = self.render_layers(nested, nested_t, m, out_w, out_h, opts, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(buf)
    }

    /// 他レイヤーをエフェクトのマップとして使うための画像（`w`×`h` に引き伸ばしたソース）。
    #[allow(clippy::too_many_arguments)]
    fn map_image(
        &self,
        comp: &Composition,
        id: &LayerId,
        t: Time,
        w: usize,
        h: usize,
        opts: RenderOptions,
        depth: usize,
    ) -> Option<FrameBuf> {
        let layer = comp.layer(id)?;
        if matches!(layer.kind, LayerKind::Shape { .. } | LayerKind::Text { .. })
            || (matches!(layer.kind, LayerKind::Precomp { .. }) && layer.collapse)
        {
            // 寸法を持たないレイヤーは、コンポ全体をバッファに合わせた座標で描く
            let fit = Affine::scale(w as f64 / comp.width as f64, h as f64 / comp.height as f64);
            let m = fit.then_after(&world_matrix(comp, layer, t));
            return self.comp_space_raw(comp, layer, t, m, w, h, opts, depth).ok();
        }
        let content = self.raw_content(comp, layer, t, opts, depth, false).ok()??;
        let stretch = |buf: &FrameBuf| {
            if buf.width == w && buf.height == h {
                buf.clone()
            } else {
                let fit = Affine::scale(w as f64 / buf.width as f64, h as f64 / buf.height as f64);
                tile_to_buf(rasterize(&Source::Image(buf), &fit, w, h, 1.0), w, h)
            }
        };
        Some(match content {
            Content::Solid { color, .. } => {
                let mut b = FrameBuf::new(w, h);
                b.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&color));
                b
            }
            Content::Image { buf, .. } => stretch(&buf),
            Content::Shared { buf, .. } => stretch(&buf),
        })
    }

    /// レイヤーのエフェクトを順に適用する。`scale` はバッファ画素 / レイヤー座標 1 単位、
    /// `mask_m` はレイヤー座標 → バッファ画素、`at_time` は別時刻のエフェクト前の絵、
    /// `below` は下のレイヤーまでの合成結果（出力画素）と、元のバッファ画素 → 出力画素の変換。
    #[allow(clippy::too_many_arguments)]
    fn apply_effects(
        &self,
        _orig: &Composition,
        comp: &Composition,
        layer: &Layer,
        t: Time,
        mut buf: FrameBuf,
        scale: f32,
        mask_m: &Affine,
        at_time: &dyn Fn(Time) -> Option<FrameBuf>,
        opts: RenderOptions,
        depth: usize,
        below: Option<(&FrameBuf, Affine)>,
    ) -> Result<(FrameBuf, [f64; 2]), RenderError> {
        let masks: Vec<BezPath> = layer.masks.iter().map(|k| transform_path(&k.path.eval(t), mask_m)).collect();
        let (w, h) = (buf.width, buf.height);
        let fetch_layer = |id: &LayerId| self.map_image(comp, id, t, w, h, opts, depth);
        let audio = |id: &LayerId, len: usize| match self.audio {
            Some(a) => a.window(self.project, &comp.id, id, t, len),
            None => Vec::new(),
        };
        // レイヤーごとに違う乱数になるよう、ID から種を作る
        let seed = layer.id.0.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
        let active: Vec<(&znd_core::EffectInstance, znd_core::EvalParams)> =
            layer.effects.iter().filter(|e| e.enabled).map(|fx| (fx, fx.eval(t))).collect();
        let env = effects::AccelEnv { scale, time_seconds: t.seconds(), cx: &self.cx };
        let offset = std::cell::Cell::new([0.0f64; 2]);
        // 下のレイヤーまでの絵を、今のバッファ（w×h、左上は元のバッファの offset）の画素に写す
        let fetch_below = |w: usize, h: usize| -> Option<FrameBuf> {
            let (canvas, m) = below?;
            let o = offset.get();
            let mut out = FrameBuf::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let (sx, sy) = m.apply(x as f64 + 0.5 + o[0], y as f64 + 0.5 + o[1]);
                    let c = sample_bilinear(canvas, sx, sy);
                    let i = (y * w + x) * 4;
                    out.data[i..i + 4].copy_from_slice(&c);
                }
            }
            Some(out)
        };
        let (layer_time, layer_duration) = ((t - layer.in_point).seconds(), (layer.out_point - layer.in_point).seconds());
        let on_gpu = |i: usize| match self.accel {
            Some(a) => a.supports(&effects::AccelOp { kind: &active[i].0.kind, params: &active[i].1 }, &env, w, h),
            None => false,
        };
        let mut i = 0;
        while i < active.len() {
            // GPU で処理できるエフェクトが続く区間は、まとめて 1 回で処理する
            if on_gpu(i) {
                let mut end = i + 1;
                while end < active.len() && on_gpu(end) {
                    end += 1;
                }
                let ops: Vec<effects::AccelOp> =
                    active[i..end].iter().map(|(fx, params)| effects::AccelOp { kind: &fx.kind, params }).collect();
                match self.accel.expect("on_gpu implies an accelerator").run(&ops, buf, &env) {
                    Ok(done) => {
                        buf = done;
                        i = end;
                        continue;
                    }
                    // 失敗したら、この区間は CPU で処理する
                    Err(original) => buf = original,
                }
            }
            let (fx, params) = (active[i].0, &active[i].1);
            i += 1;
            let eval_at = |at: Time| fx.eval(at);
            let ctx = FxContext {
                time: t,
                frame_rate: comp.frame_rate,
                scale,
                cx: &self.cx,
                fetch_layer: &fetch_layer,
                fetch_self: at_time,
                fetch_below: &fetch_below,
                audio: &audio,
                eval_at: &eval_at,
                masks: &masks,
                seed,
                offset: &offset,
                layer_time,
                layer_duration,
                comp_size: [comp.width, comp.height],
            };
            buf = effects::apply(&fx.kind, params, buf, &ctx);
        }
        Ok((buf, offset.get()))
    }
}

/// 大きさの変わった画像を、元の `w`×`h` のバッファへ `offset`（元の画素座標での左上）の位置で貼り直す。
pub(crate) fn paste_at(buf: &FrameBuf, offset: [f64; 2], w: usize, h: usize) -> FrameBuf {
    let mut out = FrameBuf::new(w, h);
    let (ox, oy) = (offset[0].round() as isize, offset[1].round() as isize);
    for y in 0..buf.height {
        let dy = y as isize + oy;
        if dy < 0 || dy >= h as isize {
            continue;
        }
        let (x0, x1) = ((-ox).max(0) as usize, (w as isize - ox).clamp(0, buf.width as isize) as usize);
        if x0 >= x1 {
            continue;
        }
        let src = &buf.data[(y * buf.width + x0) * 4..(y * buf.width + x1) * 4];
        let d = (dy as usize * w + (x0 as isize + ox) as usize) * 4;
        out.data[d..d + src.len()].copy_from_slice(src);
    }
    out
}

fn tile_to_buf(tile: Option<Tile>, w: usize, h: usize) -> FrameBuf {
    let mut out = FrameBuf::new(w, h);
    if let Some(t) = tile {
        for row in 0..t.h {
            let dst = ((t.y0 + row) * w + t.x0) * 4;
            out.data[dst..dst + t.w * 4].copy_from_slice(&t.data[row * t.w * 4..(row + 1) * t.w * 4]);
        }
    }
    out
}

/// 8/16bpc モード: タイルが触れた範囲を 0..1 に収める（整数バッファと同じく、範囲外の値は保持しない）。
fn clamp_region(canvas: &mut FrameBuf, tile: &Tile) {
    let w = canvas.width;
    for row in 0..tile.h {
        let i = ((tile.y0 + row) * w + tile.x0) * 4;
        canvas.data[i..i + tile.w * 4].iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    }
}

/// 8/16bpc モード: 表示値の段数に量子化する。
fn quantize(canvas: &mut FrameBuf, depth: BitDepth) {
    let levels = match depth {
        BitDepth::B8 => 255.0,
        BitDepth::B16 => 32768.0,
        BitDepth::F32 => return,
    };
    for px in canvas.data.chunks_exact_mut(4) {
        let a = px[3];
        if a <= 0.0 {
            continue;
        }
        for v in &mut px[..3] {
            let d = crate::color::linear_to_srgb((*v / a).clamp(0.0, 1.0));
            *v = crate::color::srgb_to_linear((d * levels).round() / levels) * a;
        }
    }
}

/// タイルにトラックマットを掛ける。`matte` が無い（空）場合はマット値 0 として扱う。
fn apply_matte(tile: &mut Tile, matte: Option<&Tile>, mode: MatteMode) {
    let (use_luma, invert) = match mode {
        MatteMode::Alpha => (false, false),
        MatteMode::AlphaInverted => (false, true),
        MatteMode::Luma => (true, false),
        MatteMode::LumaInverted => (true, true),
    };
    let (x0, y0, w) = (tile.x0, tile.y0, tile.w);
    for (i, px) in tile.data.chunks_exact_mut(4).enumerate() {
        let m = matte.map(|m| m.pixel(x0 + i % w, y0 + i / w)).unwrap_or([0.0; 4]);
        // ルミナンスマットはプリマルチプライド値の輝度（= 黒背景に置いたときの明るさ）
        let f = if use_luma { luma([m[0], m[1], m[2]]).clamp(0.0, 1.0) } else { m[3] };
        let f = if invert { 1.0 - f } else { f };
        px.iter_mut().for_each(|v| *v *= f);
    }
}

/// 出力画素の座標（画素の中心は +0.5）で双線形に読む。範囲の外は透明。
fn sample_bilinear(buf: &FrameBuf, x: f64, y: f64) -> [f32; 4] {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
    let at = |xi: f64, yi: f64| -> [f32; 4] {
        if xi < 0.0 || yi < 0.0 || xi >= buf.width as f64 || yi >= buf.height as f64 {
            return [0.0; 4];
        }
        let i = (yi as usize * buf.width + xi as usize) * 4;
        [buf.data[i], buf.data[i + 1], buf.data[i + 2], buf.data[i + 3]]
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        top + (bot - top) * ty
    })
}
