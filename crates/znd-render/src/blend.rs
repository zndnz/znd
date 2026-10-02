//! 描画モード。入力・出力ともプリマルチプライド。

use crate::color::{linear_to_srgb, luma, srgb_to_linear};
use crate::frame::{FrameBuf, Tile};
use crate::raster::rows_per_task;
use rayon::prelude::*;
use znd_core::BlendMode;

type Rgb = [f32; 3];

/// `d`（下）に `s`（上）を `mode` で合成する。
#[inline]
pub fn blend_pixel(d: [f32; 4], s: [f32; 4], mode: BlendMode) -> [f32; 4] {
    let (sa, da) = (s[3], d[3]);
    match mode {
        BlendMode::Normal => {
            let k = 1.0 - sa;
            [s[0] + d[0] * k, s[1] + d[1] * k, s[2] + d[2] * k, sa + da * k]
        }
        BlendMode::Add => [s[0] + d[0], s[1] + d[1], s[2] + d[2], sa + da - sa * da],
        // 乗算とスクリーンはプリマルチプライドのまま閉じた式で書ける（除算が要らない）
        BlendMode::Multiply => {
            let (ks, kd) = (1.0 - da, 1.0 - sa);
            let f = |i: usize| s[i] * d[i] + s[i] * ks + d[i] * kd;
            [f(0), f(1), f(2), sa + da - sa * da]
        }
        BlendMode::Screen => {
            let f = |i: usize| s[i] + d[i] - s[i] * d[i];
            [f(0), f(1), f(2), f(3)]
        }
        BlendMode::AlphaAdd => [s[0] + d[0], s[1] + d[1], s[2] + d[2], (sa + da).min(1.0)],
        BlendMode::StencilAlpha => scale(d, sa),
        BlendMode::StencilLuma => scale(d, luma([s[0], s[1], s[2]]).clamp(0.0, 1.0)),
        BlendMode::SilhouetteAlpha => scale(d, 1.0 - sa),
        BlendMode::SilhouetteLuma => scale(d, 1.0 - luma([s[0], s[1], s[2]]).clamp(0.0, 1.0)),
        _ => {
            if sa <= 0.0 {
                return d;
            }
            if da <= 0.0 {
                return s;
            }
            let cs = [s[0] / sa, s[1] / sa, s[2] / sa];
            let cb = [d[0] / da, d[1] / da, d[2] / da];
            let b = mix(cb, cs, mode);
            let both = sa * da;
            let mut out = [0.0; 4];
            for i in 0..3 {
                out[i] = (1.0 - da) * s[i] + (1.0 - sa) * d[i] + both * b[i];
            }
            out[3] = sa + da - both;
            out
        }
    }
}

fn scale(p: [f32; 4], k: f32) -> [f32; 4] {
    [p[0] * k, p[1] * k, p[2] * k, p[3] * k]
}

/// 非プリマルチプライド色どうしの混合関数 B(cb, cs)。
fn mix(cb: Rgb, cs: Rgb, mode: BlendMode) -> Rgb {
    use BlendMode::*;
    // HDR 値でも破綻しないモードはそのまま、それ以外は 0..1 に収めてから計算する。
    let hdr_safe = matches!(mode, Darken | Lighten | Difference | Subtract | DarkerColor | LighterColor);
    let (cb, cs) = if hdr_safe { (cb, cs) } else { (clamp01(cb), clamp01(cs)) };
    match mode {
        DarkerColor => {
            if luma(cs) < luma(cb) {
                cs
            } else {
                cb
            }
        }
        LighterColor => {
            if luma(cs) > luma(cb) {
                cs
            } else {
                cb
            }
        }
        Hue => set_lum(set_sat(cs, sat(cb)), luma(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), luma(cb)),
        Color => set_lum(cs, luma(cb)),
        Luminosity => set_lum(cb, luma(cs)),
        _ => [channel(cb[0], cs[0], mode), channel(cb[1], cs[1], mode), channel(cb[2], cs[2], mode)],
    }
}

fn clamp01(c: Rgb) -> Rgb {
    [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)]
}

fn dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}

fn burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b * 2.0 * s
    } else {
        let s2 = 2.0 * s - 1.0;
        b + s2 - b * s2
    }
}

#[inline(always)]
fn channel(b: f32, s: f32, mode: BlendMode) -> f32 {
    use BlendMode::*;
    match mode {
        Darken => b.min(s),
        Lighten => b.max(s),
        ColorDodge => dodge(b, s),
        ColorBurn => burn(b, s),
        LinearBurn => (b + s - 1.0).max(0.0),
        Overlay => hard_light(s, b),
        HardLight => hard_light(b, s),
        SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let g = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                b + (2.0 * s - 1.0) * (g - b)
            }
        }
        LinearLight => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
        VividLight => {
            if s <= 0.5 {
                burn(b, 2.0 * s)
            } else {
                dodge(b, 2.0 * s - 1.0)
            }
        }
        PinLight => {
            if s <= 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * s - 1.0)
            }
        }
        HardMix => {
            if b + s >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        Difference => (b - s).abs(),
        Exclusion => b + s - 2.0 * b * s,
        Subtract => (b - s).max(0.0),
        Divide => {
            if s <= 0.0 {
                if b > 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                (b / s).min(1.0)
            }
        }
        // 上位の match で処理済みのモード
        _ => s,
    }
}

fn sat(c: Rgb) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn clip_color(c: Rgb) -> Rgb {
    let l = luma(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        for v in &mut out {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 {
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    out
}

fn set_lum(c: Rgb, l: f32) -> Rgb {
    let d = l - luma(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn set_sat(c: Rgb, s: f32) -> Rgb {
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    if x <= n {
        return [0.0; 3];
    }
    let k = s / (x - n);
    [(c[0] - n) * k, (c[1] - n) * k, (c[2] - n) * k]
}

/// プリマルチプライドのリニア画素を、プリマルチプライドの表示ガンマ画素に変換する。
fn to_display(p: [f32; 4]) -> [f32; 4] {
    convert(p, linear_to_srgb)
}

fn to_linear(p: [f32; 4]) -> [f32; 4] {
    convert(p, srgb_to_linear)
}

fn convert(p: [f32; 4], f: fn(f32) -> f32) -> [f32; 4] {
    let a = p[3];
    if a <= 0.0 {
        return p;
    }
    [f(p[0] / a) * a, f(p[1] / a) * a, f(p[2] / a) * a, a]
}

fn is_stencil(mode: BlendMode) -> bool {
    matches!(mode, BlendMode::StencilAlpha | BlendMode::StencilLuma)
}

/// タイルをキャンバスに合成する。`linear == false` のときは表示ガンマ空間で混合する。
pub fn blend_tile(canvas: &mut FrameBuf, tile: &Tile, mode: BlendMode, linear: bool) {
    let cw = canvas.width;
    if is_stencil(mode) {
        // ステンシルはレイヤーの外側（アルファ 0）も下の絵を消す。
        canvas.data.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
            let inside_y = y >= tile.y0 && y < tile.y0 + tile.h;
            for (x, px) in row.chunks_exact_mut(4).enumerate() {
                if !(inside_y && x >= tile.x0 && x < tile.x0 + tile.w) {
                    px.fill(0.0);
                }
            }
        });
    }
    if tile.w == 0 || tile.h == 0 {
        return;
    }
    // 完全に透明なソース画素は、ステンシル以外のどのモードでも下の絵を変えない
    let skip = !is_stencil(mode);
    // モードの分岐を画素ループの外に出す（頻出モードは専用ループに単相化される）
    match mode {
        BlendMode::Normal => run(canvas, tile, linear, skip, |d, s| blend_pixel(d, s, BlendMode::Normal)),
        BlendMode::Add => run(canvas, tile, linear, skip, |d, s| blend_pixel(d, s, BlendMode::Add)),
        BlendMode::Screen => run(canvas, tile, linear, skip, |d, s| blend_pixel(d, s, BlendMode::Screen)),
        BlendMode::Multiply => run(canvas, tile, linear, skip, |d, s| blend_pixel(d, s, BlendMode::Multiply)),
        _ => run(canvas, tile, linear, skip, |d, s| blend_pixel(d, s, mode)),
    }
}

fn run(canvas: &mut FrameBuf, tile: &Tile, linear: bool, skip_transparent: bool, f: impl Fn([f32; 4], [f32; 4]) -> [f32; 4] + Sync) {
    let cw = canvas.width;
    let rows = &mut canvas.data[tile.y0 * cw * 4..(tile.y0 + tile.h) * cw * 4];
    rows.par_chunks_mut(cw * 4).zip(tile.data.par_chunks(tile.w * 4)).with_min_len(rows_per_task(tile.w)).for_each(|(crow, trow)| {
        let crow = &mut crow[tile.x0 * 4..(tile.x0 + tile.w) * 4];
        for (d, s) in crow.chunks_exact_mut(4).zip(trow.chunks_exact(4)) {
            let s = [s[0], s[1], s[2], s[3]];
            if skip_transparent && s == [0.0; 4] {
                continue;
            }
            let dp = [d[0], d[1], d[2], d[3]];
            let out = if linear { f(dp, s) } else { to_linear(f(to_display(dp), to_display(s))) };
            d.copy_from_slice(&out);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn normal_over() {
        let d = [0.0, 0.0, 1.0, 1.0];
        let s = [0.5, 0.0, 0.0, 0.5];
        assert!(close(blend_pixel(d, s, BlendMode::Normal), [0.5, 0.0, 0.5, 1.0]));
        // 透明な下地にはソースがそのまま残る
        assert!(close(blend_pixel([0.0; 4], s, BlendMode::Multiply), s));
    }

    #[test]
    fn separable_modes_on_opaque_pixels() {
        let d = [0.2, 0.5, 0.8, 1.0];
        let s = [0.6, 0.5, 0.1, 1.0];
        let cases: [(BlendMode, [f32; 3]); 9] = [
            (BlendMode::Multiply, [0.12, 0.25, 0.08]),
            (BlendMode::Screen, [0.68, 0.75, 0.82]),
            (BlendMode::Add, [0.8, 1.0, 0.9]),
            (BlendMode::Darken, [0.2, 0.5, 0.1]),
            (BlendMode::Lighten, [0.6, 0.5, 0.8]),
            (BlendMode::Difference, [0.4, 0.0, 0.7]),
            (BlendMode::Overlay, [0.24, 0.5, 0.64]),
            (BlendMode::HardLight, [0.36, 0.5, 0.16]),
            (BlendMode::Exclusion, [0.56, 0.5, 0.74]),
        ];
        for (mode, e) in cases {
            let out = blend_pixel(d, s, mode);
            assert!(close(out, [e[0], e[1], e[2], 1.0]), "{mode:?}: {out:?}");
        }
    }

    #[test]
    fn every_mode_is_finite_and_keeps_alpha_in_range() {
        let samples = [[0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0], [0.3, 0.1, 0.05, 0.4], [2.0, 0.5, 0.0, 1.0]];
        for mode in BlendMode::ALL {
            for d in samples {
                for s in samples {
                    let o = blend_pixel(d, s, mode);
                    assert!(o.iter().all(|v| v.is_finite()), "{mode:?} {d:?} {s:?} -> {o:?}");
                    assert!((0.0..=1.0 + 1e-6).contains(&o[3]), "{mode:?} alpha {o:?}");
                }
            }
        }
    }

    #[test]
    fn luminosity_keeps_backdrop_hue() {
        let d = [1.0, 0.0, 0.0, 1.0];
        let s = [0.5, 0.5, 0.5, 1.0];
        let o = blend_pixel(d, s, BlendMode::Luminosity);
        assert!((luma([o[0], o[1], o[2]]) - 0.5).abs() < 1e-4);
        assert!(o[0] > o[1] && (o[1] - o[2]).abs() < 1e-5);
    }

    #[test]
    fn stencil_clears_outside_tile() {
        let mut canvas = FrameBuf::new(4, 4);
        canvas.data.fill(1.0);
        let tile = Tile { x0: 1, y0: 1, w: 2, h: 2, data: [0.5f32; 16].to_vec() };
        blend_tile(&mut canvas, &tile, BlendMode::StencilAlpha, true);
        assert_eq!(canvas.pixel(0, 0), [0.0; 4]);
        assert_eq!(canvas.pixel(1, 1), [0.5; 4]);
        assert_eq!(canvas.pixel(3, 2), [0.0; 4]);
    }

    #[test]
    fn gamma_space_blend_differs_from_linear() {
        let tile = Tile { x0: 0, y0: 0, w: 1, h: 1, data: vec![0.5, 0.5, 0.5, 0.5] };
        let mut lin = FrameBuf::new(1, 1);
        lin.set_pixel(0, 0, [0.0, 0.0, 0.0, 1.0]);
        let mut gam = lin.clone();
        blend_tile(&mut lin, &tile, BlendMode::Normal, true);
        blend_tile(&mut gam, &tile, BlendMode::Normal, false);
        assert!((lin.pixel(0, 0)[0] - 0.5).abs() < 1e-6);
        // 表示値 1.0 と 0.0 の 50% 混合 = 表示値 0.5 = リニア 0.214
        assert!((gam.pixel(0, 0)[0] - 0.214).abs() < 1e-3);
    }
}
