//! レイヤーソースをアフィン変換して出力キャンバス上のタイルにラスタライズする。
//!
//! 矩形の縁は「出力画素のフットプリントとソース矩形の重なり面積」で解析的に
//! アンチエイリアスする。画像の色はバイリニア（端はクランプ）で、縮小時は
//! 最大 4×4 のスーパーサンプリングを行う。

use crate::frame::{take_zeroed, FrameBuf, Tile};
use rayon::prelude::*;
use znd_core::Affine;

pub enum Source<'a> {
    /// 単色の矩形。`color` はプリマルチプライド・リニア。
    Solid {
        width: f32,
        height: f32,
        color: [f32; 4],
    },
    Image(&'a FrameBuf),
}

impl Source<'_> {
    fn size(&self) -> (f32, f32) {
        match self {
            Source::Solid { width, height, .. } => (*width, *height),
            Source::Image(img) => (img.width as f32, img.height as f32),
        }
    }
}

const MAX_SUPERSAMPLE: f32 = 4.0;

/// `m` はソース画素座標 → 出力画素座標。何も描かれない場合は `None`。
pub fn rasterize(src: &Source, m: &Affine, out_w: usize, out_h: usize, opacity: f32) -> Option<Tile> {
    let (sw, sh) = src.size();
    if sw <= 0.0 || sh <= 0.0 || opacity <= 0.0 {
        return None;
    }
    let inv = m.inverse()?;

    // 出力上のバウンディングボックス
    let corners = [(0.0, 0.0), (sw as f64, 0.0), (0.0, sh as f64), (sw as f64, sh as f64)].map(|(x, y)| m.apply(x, y));
    let min_x = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor() - 1.0;
    let max_x = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil() + 1.0;
    let min_y = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor() - 1.0;
    let max_y = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil() + 1.0;
    let x0 = min_x.clamp(0.0, out_w as f64) as usize;
    let x1 = max_x.clamp(0.0, out_w as f64) as usize;
    let y0 = min_y.clamp(0.0, out_h as f64) as usize;
    let y1 = max_y.clamp(0.0, out_h as f64) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);

    // 出力 1 画素がソース空間で占める幅（軸平行の外接矩形）
    let fx = (inv.a.abs() + inv.c.abs()) as f32;
    let fy = (inv.b.abs() + inv.d.abs()) as f32;
    // 出力 1 画素ぶんのステップ長からスーパーサンプル数を決める
    let nx = ((inv.a.hypot(inv.b)) as f32).ceil().clamp(1.0, MAX_SUPERSAMPLE) as usize;
    let ny = ((inv.c.hypot(inv.d)) as f32).ceil().clamp(1.0, MAX_SUPERSAMPLE) as usize;

    let mut data = take_zeroed(w * h * 4);
    data.par_chunks_mut(w * 4).with_min_len(rows_per_task(w)).enumerate().for_each(|(row, out)| {
        let py = (y0 + row) as f64 + 0.5;
        // この行でソース矩形（＋フットプリント）に掛かる列の範囲だけを処理する。
        // 回転したレイヤーではバウンディングボックスの大半が空なので効果が大きい。
        let (ax, bx) = span(inv.a, inv.c * py + inv.tx, -0.5 * fx as f64, sw as f64 + 0.5 * fx as f64);
        let (ay, by) = span(inv.b, inv.d * py + inv.ty, -0.5 * fy as f64, sh as f64 + 0.5 * fy as f64);
        let first = ((ax.max(ay) - x0 as f64 - 1.5).floor().max(0.0) as usize).min(w);
        let last = ((bx.min(by) - x0 as f64 + 1.5).ceil().max(0.0) as usize).min(w);
        if first >= last {
            return;
        }
        for (col, px) in out.chunks_exact_mut(4).enumerate().take(last).skip(first) {
            let pxc = (x0 + col) as f64 + 0.5;
            let (sx, sy) = inv.apply(pxc, py);
            let (sx, sy) = (sx as f32, sy as f32);
            let cov = overlap(sx, fx, sw) * overlap(sy, fy, sh) * opacity;
            if cov <= 0.0 {
                continue;
            }
            let c = match src {
                Source::Solid { color, .. } => *color,
                Source::Image(img) if nx == 1 && ny == 1 => bilinear(img, sx, sy),
                Source::Image(img) => {
                    let mut acc = [0.0f32; 4];
                    for j in 0..ny {
                        let oy = (j as f64 + 0.5) / ny as f64 - 0.5;
                        for i in 0..nx {
                            let ox = (i as f64 + 0.5) / nx as f64 - 0.5;
                            let (ax, ay) = inv.apply(pxc + ox, py + oy);
                            let s = bilinear(img, ax as f32, ay as f32);
                            for k in 0..4 {
                                acc[k] += s[k];
                            }
                        }
                    }
                    let n = (nx * ny) as f32;
                    [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]
                }
            };
            px[0] = c[0] * cov;
            px[1] = c[1] * cov;
            px[2] = c[2] * cov;
            px[3] = c[3] * cov;
        }
    });
    Some(Tile { x0, y0, w, h, data })
}

/// 小さなタイルを行単位で細かく並列化するとタスク分割のコストが勝つので、
/// 1 タスクあたり一定の画素数になるよう行数をまとめる。
pub(crate) fn rows_per_task(width: usize) -> usize {
    (4_096 / width.max(1)).max(1)
}

/// `lo < k·x + c < hi` を満たす x の範囲。
fn span(k: f64, c: f64, lo: f64, hi: f64) -> (f64, f64) {
    if k.abs() < 1e-12 {
        return if c > lo && c < hi { (f64::MIN, f64::MAX) } else { (f64::MAX, f64::MIN) };
    }
    let (a, b) = ((lo - c) / k, (hi - c) / k);
    (a.min(b), a.max(b))
}

/// 中心 `c`・幅 `f` の区間と [0, size] の重なりの割合。
fn overlap(c: f32, f: f32, size: f32) -> f32 {
    let lo = (c - 0.5 * f).max(0.0);
    let hi = (c + 0.5 * f).min(size);
    ((hi - lo) / f).clamp(0.0, 1.0)
}

/// ソース座標（画素の中心が +0.5）でのバイリニアサンプル。画像外は端の画素を延長する。
fn bilinear(img: &FrameBuf, sx: f32, sy: f32) -> [f32; 4] {
    let u = (sx - 0.5).clamp(0.0, (img.width - 1) as f32);
    let v = (sy - 0.5).clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (u as usize, v as usize);
    let x1 = (x0 + 1).min(img.width - 1);
    let y1 = (y0 + 1).min(img.height - 1);
    let (fu, fv) = (u - x0 as f32, v - y0 as f32);
    let at = |x: usize, y: usize| {
        let i = (y * img.width + x) * 4;
        &img.data[i..i + 4]
    };
    let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut out = [0.0f32; 4];
    for k in 0..4 {
        let top = p00[k] + (p10[k] - p00[k]) * fu;
        let bot = p01[k] + (p11[k] - p01[k]) * fu;
        out[k] = top + (bot - top) * fv;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    fn canvas_alpha(tile: &Tile, x: usize, y: usize) -> f32 {
        tile.pixel(x, y)[3]
    }

    #[test]
    fn pixel_aligned_solid_has_hard_edges() {
        let src = Source::Solid { width: 4.0, height: 2.0, color: WHITE };
        let t = rasterize(&src, &Affine::translate(3.0, 5.0), 16, 16, 1.0).unwrap();
        assert_eq!(canvas_alpha(&t, 2, 5), 0.0);
        assert_eq!(canvas_alpha(&t, 3, 5), 1.0);
        assert_eq!(canvas_alpha(&t, 6, 6), 1.0);
        assert_eq!(canvas_alpha(&t, 7, 6), 0.0);
        assert_eq!(canvas_alpha(&t, 3, 7), 0.0);
    }

    #[test]
    fn subpixel_offset_gives_fractional_coverage() {
        let src = Source::Solid { width: 4.0, height: 4.0, color: WHITE };
        let t = rasterize(&src, &Affine::translate(2.25, 2.0), 16, 16, 1.0).unwrap();
        assert!((canvas_alpha(&t, 2, 3) - 0.75).abs() < 1e-5);
        assert!((canvas_alpha(&t, 6, 3) - 0.25).abs() < 1e-5);
        // 被覆の総和は面積に一致する
        let total: f32 = t.data.chunks_exact(4).map(|p| p[3]).sum();
        assert!((total - 16.0).abs() < 1e-3);
    }

    #[test]
    fn rotated_and_minified_solid_conserves_area() {
        let src = Source::Solid { width: 40.0, height: 20.0, color: WHITE };
        let m = Affine::translate(32.0, 32.0).then_after(&Affine::rotate(30.0)).then_after(&Affine::scale(0.5, 0.5));
        let t = rasterize(&src, &m, 64, 64, 1.0).unwrap();
        let total: f32 = t.data.chunks_exact(4).map(|p| p[3]).sum();
        assert!((total - 200.0).abs() / 200.0 < 0.03, "area {total}");
    }

    #[test]
    fn identity_image_copy_is_exact_and_clipped() {
        let mut img = FrameBuf::new(3, 2);
        for (i, v) in img.data.iter_mut().enumerate() {
            *v = i as f32 / 24.0;
        }
        let t = rasterize(&Source::Image(&img), &Affine::translate(1.0, 1.0), 3, 3, 1.0).unwrap();
        assert_eq!(t.pixel(1, 1), img.pixel(0, 0));
        assert_eq!(t.pixel(2, 2), img.pixel(1, 1));
        assert_eq!(t.pixel(0, 0), [0.0; 4]);
        assert!(rasterize(&Source::Image(&img), &Affine::translate(50.0, 0.0), 3, 3, 1.0).is_none());
    }

    #[test]
    fn minified_image_averages_texels() {
        // 2x2 の市松を 1/2 に縮小 → 1 画素に平均される
        let mut img = FrameBuf::new(2, 2);
        img.set_pixel(0, 0, WHITE);
        img.set_pixel(1, 1, WHITE);
        let t = rasterize(&Source::Image(&img), &Affine::scale(0.5, 0.5), 1, 1, 1.0).unwrap();
        assert!((t.pixel(0, 0)[3] - 0.5).abs() < 1e-5);
    }
}
