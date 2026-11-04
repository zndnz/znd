//! エフェクトの実装。定義（パラメータ表）は `znd_core::effect`。
//!
//! すべて CPU 実装で、行単位に並列化している。px 単位のパラメータは `FxContext::scale`
//! （プレビュー解像度の縮小率）を掛けてから使うので、解像度を落としても見た目が変わらない。
//!
//! 色調補正系は「表示値（sRGB 符号化後）」で計算する。レベルやトーンカーブの数値が
//! 既存ツールと同じ感覚で効くようにするため。

mod blur;
mod color;
mod distort;
mod generate;
mod keying;
mod light;
mod particles;
mod plugin;
mod stylize;
mod time;

use crate::color::{linear_to_srgb, srgb_to_linear, ColorXf};
use crate::frame::FrameBuf;
use rayon::prelude::*;
use znd_core::effect::EvalParams;
use znd_core::shape::BezPath;
use znd_core::{FrameRate, LayerId, Time};

pub struct FxContext<'a> {
    pub time: Time,
    pub frame_rate: FrameRate,
    /// バッファ画素 / レイヤー座標 1 単位。
    pub scale: f32,
    pub cx: &'a ColorXf,
    /// 他レイヤーのソース画像を、このバッファと同じ大きさで取得する。
    pub fetch_layer: &'a dyn Fn(&LayerId) -> Option<FrameBuf>,
    /// このレイヤーの（エフェクト適用前の）画像を別の時刻で取得する。
    pub fetch_self: &'a dyn Fn(Time) -> Option<FrameBuf>,
    /// 指定レイヤーの音声（モノラル 48kHz）を、現在時刻を中心に `len` サンプル取得する。
    pub audio: &'a dyn Fn(&LayerId, usize) -> Vec<f32>,
    /// このエフェクトのパラメータを別の時刻で評価する（粒子の発生位置など）。
    pub eval_at: &'a dyn Fn(Time) -> EvalParams,
    /// レイヤーのマスクパス（バッファ画素座標）。「線」「落書き」が使う。
    pub masks: &'a [BezPath],
    /// レイヤーごとに異なる乱数の種。
    pub seed: u32,
    /// 画像の大きさを変えるエフェクト（プラグイン）が、出力の左上を元のバッファのどこに置いたか（画素）。
    /// エフェクトの列を通して足し込まれ、合成時にレイヤー上の置き場所へ反映される。
    pub offset: &'a std::cell::Cell<[f64; 2]>,
    /// レイヤーのイン点からの経過秒。
    pub layer_time: f64,
    /// レイヤーの長さ（イン点〜アウト点、秒）。
    pub layer_duration: f64,
}

/// エフェクトを 1 つ適用する。未知の種類は何もしない。
pub fn apply(kind: &str, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    match kind {
        "gaussian_blur" | "directional_blur" | "radial_blur" | "zoom_blur" | "lens_blur" | "compound_blur" => {
            blur::apply(kind, p, buf, ctx)
        }
        "glow" | "bloom" | "lens_flare" | "light_rays" | "drop_shadow" => light::apply(kind, p, buf, ctx),
        "displacement_map" | "turbulent_displace" | "wave_warp" | "polar_coordinates" | "mesh_warp" | "liquify" | "mirror"
        | "motion_tile" => distort::apply(kind, p, buf, ctx),
        "fractal_noise" | "gradient_ramp" | "cell_pattern" | "checkerboard" | "audio_spectrum" | "audio_waveform" | "stroke"
        | "scribble" => generate::apply(kind, p, buf, ctx),
        "echo" | "afterimage" | "pixel_motion_blur" | "time_displacement" => time::apply(kind, p, buf, ctx),
        "glitch" | "rgb_split" | "posterize" | "halftone" | "mosaic" | "find_edges" | "noise" | "film_grain" | "scanlines" => {
            stylize::apply(kind, p, buf, ctx)
        }
        "levels"
        | "curves"
        | "hue_saturation"
        | "color_balance"
        | "lut"
        | "tritone"
        | "gradient_map"
        | "shadow_highlight"
        | "exposure"
        | "brightness_contrast"
        | "invert" => color::apply(kind, p, buf, ctx),
        "chroma_key" | "luma_key" | "matte_choker" => keying::apply(kind, p, buf, ctx),
        "particles" => particles::apply(p, buf, ctx),
        "plugin" => plugin::apply(p, buf, ctx),
        k if crate::plugins::is_plugin_kind(k) => crate::plugins::apply(k, p, buf, ctx),
        // エクスプレッション制御などは画像を変えない
        _ => buf,
    }
}

pub use blur::{iris_blur, sigma_for};
pub use color::{set_base_dir, Lut3d};
pub use plugin::{probe as probe_plugin, PluginInfo, PLUGIN_ABI_VERSION};

// ---------- 共通ユーティリティ ----------

pub(crate) fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

pub(crate) fn hash3(a: u32, b: u32, c: u32) -> u32 {
    hash(a.wrapping_mul(0x9e37_79b1) ^ hash(b.wrapping_mul(0x85eb_ca6b) ^ hash(c)))
}

/// 0..1 の一様乱数。
pub(crate) fn rand01(a: u32, b: u32, c: u32) -> f32 {
    (hash3(a, b, c) >> 8) as f32 / (1u32 << 24) as f32
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// 3D の値ノイズ（0..1）。
pub(crate) fn noise3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (fade(x - xi), fade(y - yi), fade(z - zi));
    let (xi, yi, zi) = (xi as i32, yi as i32, zi as i32);
    let v = |dx: i32, dy: i32, dz: i32| rand01((xi + dx) as u32 ^ seed.wrapping_mul(0x27d4_eb2f), (yi + dy) as u32, (zi + dz) as u32);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(v(0, 0, 0), v(1, 0, 0), fx);
    let x10 = lerp(v(0, 1, 0), v(1, 1, 0), fx);
    let x01 = lerp(v(0, 0, 1), v(1, 0, 1), fx);
    let x11 = lerp(v(0, 1, 1), v(1, 1, 1), fx);
    lerp(lerp(x00, x10, fy), lerp(x01, x11, fy), fz)
}

/// フラクタルノイズ（0..1 付近）。`turbulent` は各オクターブの絶対値を重ねる。
pub(crate) fn fbm(x: f32, y: f32, z: f32, octaves: f32, turbulent: bool, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut norm = 0.0;
    let (mut fx, mut fy) = (x, y);
    let n = octaves.ceil().max(1.0) as u32;
    for o in 0..n {
        // 最後のオクターブは小数部ぶんだけ混ぜる（複雑度を滑らかにアニメートできる）
        let w = if o + 1 == n { 1.0 - (n as f32 - octaves) } else { 1.0 };
        let v = noise3(fx, fy, z + o as f32 * 17.3, seed.wrapping_add(o));
        let v = if turbulent { (v * 2.0 - 1.0).abs() } else { v };
        sum += v * amp * w;
        norm += amp * w;
        amp *= 0.5;
        fx *= 2.0;
        fy *= 2.0;
    }
    sum / norm.max(1e-6)
}

/// バイリニアサンプル（画素中心が +0.5）。画像外は透明。
pub(crate) fn sample(buf: &FrameBuf, x: f32, y: f32) -> [f32; 4] {
    let (u, v) = (x - 0.5, y - 0.5);
    let (x0, y0) = (u.floor(), v.floor());
    let (fu, fv) = (u - x0, v - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let (w, h) = (buf.width as isize, buf.height as isize);
    if x0 < -1 || y0 < -1 || x0 >= w || y0 >= h {
        return [0.0; 4];
    }
    let at = |x: isize, y: isize| -> [f32; 4] {
        if x < 0 || y < 0 || x >= w || y >= h {
            [0.0; 4]
        } else {
            let i = (y * w + x) as usize * 4;
            [buf.data[i], buf.data[i + 1], buf.data[i + 2], buf.data[i + 3]]
        }
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    let mut o = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * fu;
        let bot = c[k] + (d[k] - c[k]) * fu;
        o[k] = top + (bot - top) * fv;
    }
    o
}

/// バイリニアサンプル。画像外は端の画素を延長する。
pub(crate) fn sample_clamp(buf: &FrameBuf, x: f32, y: f32) -> [f32; 4] {
    sample(buf, x.clamp(0.5, buf.width as f32 - 0.5), y.clamp(0.5, buf.height as f32 - 0.5))
}

/// 端延長つきのバイリニアサンプル（3D の平面レイヤー描画用に公開）。
pub fn sample_clamped(buf: &FrameBuf, x: f32, y: f32) -> [f32; 4] {
    sample_clamp(buf, x, y)
}

/// 各画素を (x, y, 元の値) から計算し直す（並列）。
pub(crate) fn map_pixels(buf: &mut FrameBuf, f: impl Fn(usize, usize, [f32; 4]) -> [f32; 4] + Sync) {
    let w = buf.width;
    buf.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let o = f(x, y, [px[0], px[1], px[2], px[3]]);
            px.copy_from_slice(&o);
        }
    });
}

/// 新しい画像を (x, y) から生成する（並列）。
pub(crate) fn generate(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 4] + Sync) -> FrameBuf {
    let mut out = FrameBuf::new(w, h);
    map_pixels(&mut out, |x, y, _| f(x, y));
    out
}

/// 逆写像ワープ: 出力 (x, y) の値を、元画像の `f(x, y)` の位置から取ってくる。
pub(crate) fn remap(src: &FrameBuf, f: impl Fn(f32, f32) -> (f32, f32) + Sync) -> FrameBuf {
    generate(src.width, src.height, |x, y| {
        let (sx, sy) = f(x as f32 + 0.5, y as f32 + 0.5);
        sample(src, sx, sy)
    })
}

/// プリマルチプライドのリニア → ストレートの表示値（sRGB 符号化）。
pub(crate) fn to_display(px: [f32; 4]) -> [f32; 3] {
    if px[3] <= 0.0 {
        return [0.0; 3];
    }
    [linear_to_srgb(px[0] / px[3]), linear_to_srgb(px[1] / px[3]), linear_to_srgb(px[2] / px[3])]
}

pub(crate) fn from_display(c: [f32; 3], a: f32) -> [f32; 4] {
    [srgb_to_linear(c[0]) * a, srgb_to_linear(c[1]) * a, srgb_to_linear(c[2]) * a, a]
}

/// 表示値の色に対する変換を全画素に掛ける（アルファは保つ）。
pub(crate) fn map_display(buf: &mut FrameBuf, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    map_pixels(buf, |_, _, px| if px[3] <= 0.0 { px } else { from_display(f(to_display(px)), px[3]) });
}

/// 表示値での輝度。
pub(crate) fn display_luma(px: [f32; 4]) -> f32 {
    luma(to_display(px))
}

pub(crate) use crate::color::luma;

pub(crate) fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// 「元の画像とブレンド」: `blend`（%）が 100 のとき元の画像のまま。
pub(crate) fn blend_with_original(original: &FrameBuf, mut result: FrameBuf, blend: f64) -> FrameBuf {
    let t = (blend / 100.0).clamp(0.0, 1.0) as f32;
    if t > 0.0 {
        result.data.iter_mut().zip(&original.data).for_each(|(r, o)| *r += (*o - *r) * t);
    }
    result
}

pub(crate) fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x >= e0 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------- GPU などによる高速化の入口 ----------

/// 高速化バックエンドに渡すエフェクト 1 つ（時刻評価済みのパラメータ）。
pub struct AccelOp<'a> {
    pub kind: &'a str,
    pub params: &'a EvalParams,
}

/// エフェクトに共通の環境。
pub struct AccelEnv<'a> {
    /// バッファ画素 / レイヤー座標 1 単位。
    pub scale: f32,
    pub time_seconds: f64,
    pub cx: &'a ColorXf,
}

/// エフェクトの高速化バックエンド（`znd-gpu` が実装する）。
///
/// 結果は CPU 実装（このモジュール）と同じでなければならない。CPU 実装が「正解」で、
/// バックエンドはそれとの差分で検証する。
pub trait EffectAccel: Sync {
    /// このエフェクトをこのパラメータ・画像サイズで処理できるか。できないものは CPU で処理される。
    fn supports(&self, op: &AccelOp, env: &AccelEnv, width: usize, height: usize) -> bool;
    /// 連続するエフェクトをまとめて適用する（画像の転送を 1 往復で済ませるため）。
    /// 失敗したら、受け取った画像をそのまま `Err` で返す。
    fn run(&self, ops: &[AccelOp], buf: FrameBuf, env: &AccelEnv) -> Result<FrameBuf, FrameBuf>;
}
