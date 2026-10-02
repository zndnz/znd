//! 受け入れテスト共通のヘルパー。
#![allow(dead_code)]

use std::path::PathBuf;
use znd_core::*;
use znd_render::{to_rgba8, FrameBuf, NoFootage, RenderOptions, Renderer};

pub const RED: [f32; 3] = [1.0, 0.0, 0.0];
pub const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
pub const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
pub const WHITE: [f32; 3] = [1.0, 1.0, 1.0];
pub const ALPHA: RenderOptions = RenderOptions { downsample: 1, background: false, guides: true };

pub fn dur() -> Time {
    Time::from_seconds(2.0)
}

pub fn comp(id: &str, w: u32, h: u32, layers: Vec<Layer>) -> Composition {
    Composition { layers, ..Composition::new(id, id, w, h, FrameRate::FPS_30, dur()) }
}

/// コンポ中央に置いた平面。
pub fn solid(id: &str, w: u32, h: u32, color: [f32; 3], cw: u32, ch: u32) -> Layer {
    Layer::new(
        id,
        id,
        LayerKind::Solid { width: w, height: h, color },
        Transform::centered(w as f64, h as f64, cw as f64, ch as f64),
        dur(),
    )
}

/// コンポ中央を原点にしたシェイプレイヤー。
pub fn shape(id: &str, items: Vec<ShapeItem>, cw: u32, ch: u32) -> Layer {
    Layer::new(id, id, LayerKind::Shape { items }, Transform::centered(0.0, 0.0, cw as f64, ch as f64), dur())
}

pub fn text(id: &str, doc: TextDocument, cw: u32, ch: u32) -> Layer {
    Layer::new(
        id,
        id,
        LayerKind::Text { document: doc, animators: Vec::new(), path: None, per_char_3d: false },
        Transform::centered(0.0, 0.0, cw as f64, ch as f64),
        dur(),
    )
}

pub fn fx(kind: &str, w: u32, h: u32) -> EffectInstance {
    EffectInstance::new(kind, w as f64, h as f64).unwrap_or_else(|| panic!("unknown effect {kind}"))
}

pub fn project(comps: Vec<Composition>) -> Project {
    let mut p = Project::new("test");
    p.compositions = comps;
    p
}

pub fn render_at(p: &Project, id: &str, seconds: f64, opts: RenderOptions) -> FrameBuf {
    Renderer::new(p, &NoFootage).render_frame(&CompId::new(id), Time::from_seconds(seconds), opts).unwrap()
}

pub fn render(p: &Project, id: &str) -> FrameBuf {
    render_at(p, id, 0.0, ALPHA)
}

pub fn assert_px(buf: &FrameBuf, x: usize, y: usize, expect: [f32; 4], tol: f32) {
    let p = buf.pixel(x, y);
    assert!(p.iter().zip(&expect).all(|(a, b)| (a - b).abs() <= tol), "({x},{y}) = {p:?}, expected {expect:?}");
}

pub fn alpha_sum(buf: &FrameBuf) -> f64 {
    buf.data.chunks_exact(4).map(|p| p[3] as f64).sum()
}

pub fn all_finite(buf: &FrameBuf) -> bool {
    buf.data.iter().all(|v| v.is_finite())
}

/// アルファ加重のインクの重心。
pub fn centroid(buf: &FrameBuf) -> (f64, f64) {
    let (mut sx, mut sy, mut s) = (0.0, 0.0, 0.0);
    for y in 0..buf.height {
        for x in 0..buf.width {
            let a = buf.pixel(x, y)[3] as f64;
            sx += a * (x as f64 + 0.5);
            sy += a * (y as f64 + 0.5);
            s += a;
        }
    }
    (sx / s.max(1e-12), sy / s.max(1e-12))
}

/// 複数の画像を格子状に並べる（一覧の参照画像用）。
pub fn atlas(cells: &[FrameBuf], cols: usize) -> FrameBuf {
    let (cw, ch) = (cells[0].width, cells[0].height);
    let rows = cells.len().div_ceil(cols);
    let mut out = FrameBuf::new(cols * cw, rows * ch);
    for (i, c) in cells.iter().enumerate() {
        let (ox, oy) = ((i % cols) * cw, (i / cols) * ch);
        for y in 0..ch {
            for x in 0..cw {
                out.set_pixel(ox + x, oy + y, c.pixel(x, y));
            }
        }
    }
    out
}

/// 参照画像比較。既定の許容: 8bit sRGB で最大差 2、RMSE 0.25 以下。
/// 更新は `ZND_BLESS=1 cargo test`（差分を目視してからコミットすること）。
pub fn golden(name: &str, buf: &FrameBuf) {
    golden_tol(name, buf, 2, 0.25);
}

pub fn golden_tol(name: &str, buf: &FrameBuf, max_diff: i32, max_rmse: f64) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.png"));
    let actual = to_rgba8(buf, None);
    if std::env::var_os("ZND_BLESS").is_some() {
        image::save_buffer(&path, &actual, buf.width as u32, buf.height as u32, image::ExtendedColorType::Rgba8).unwrap();
        return;
    }
    let expected = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}（ZND_BLESS=1 で生成）", path.display())).to_rgba8();
    assert_eq!((expected.width() as usize, expected.height() as usize), (buf.width, buf.height), "{name}: size");
    let mut max = 0i32;
    let mut sum_sq = 0f64;
    for (a, e) in actual.iter().zip(expected.as_raw()) {
        let d = (*a as i32 - *e as i32).abs();
        max = max.max(d);
        sum_sq += (d * d) as f64;
    }
    let rmse = (sum_sq / actual.len() as f64).sqrt();
    if max > max_diff || rmse > max_rmse {
        let failed = path.with_extension("actual.png");
        image::save_buffer(&failed, &actual, buf.width as u32, buf.height as u32, image::ExtendedColorType::Rgba8).unwrap();
        panic!("{name}: 参照画像と不一致（最大差 {max}, RMSE {rmse:.3}）。出力: {}", failed.display());
    }
}
