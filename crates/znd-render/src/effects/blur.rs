//! ブラー系。

use super::{display_luma, generate, mix4, sample, sample_clamp, FxContext};
use crate::blur::{gaussian, Edge};
use crate::frame::FrameBuf;
use znd_core::effect::EvalParams;

/// 「ブラー量 R」に対するガウスの σ。R はぼけの広がりの目安（±R でほぼ収まる）。
pub fn sigma_for(radius: f64, scale: f32) -> f32 {
    (radius as f32 * scale / 2.0).max(0.0)
}

pub(crate) fn gaussian_buf(buf: &mut FrameBuf, sx: f32, sy: f32, edge: Edge) {
    let (w, h) = (buf.width, buf.height);
    gaussian(&mut buf.data, w, h, 4, sx, sy, edge);
}

/// 線分に沿って `n` 点サンプルした平均。`pos(i 0..1)` がサンプル位置を返す。
fn average_along(src: &FrameBuf, n: usize, pos: impl Fn(f32, f32, f32) -> (f32, f32) + Sync) -> FrameBuf {
    let n = n.max(1);
    generate(src.width, src.height, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut acc = [0.0f32; 4];
        for i in 0..n {
            let u = if n == 1 { 0.5 } else { i as f32 / (n - 1) as f32 };
            let (sx, sy) = pos(px, py, u);
            let s = sample(src, sx, sy);
            for k in 0..4 {
                acc[k] += s[k];
            }
        }
        acc.map(|v| v / n as f32)
    })
}

/// 絞り形状（正多角形）の内側にある標本点。総数が増えすぎないよう間引く。
fn iris_taps(radius: f32, sides: f32, rotation: f32) -> Vec<(f32, f32)> {
    let sides = sides.round().clamp(3.0, 16.0);
    let step = (radius / 8.0).max(1.0);
    let n = (radius / step).ceil() as i32;
    let apothem_ratio = (std::f32::consts::PI / sides).cos();
    let sector = std::f32::consts::TAU / sides;
    let mut taps = Vec::new();
    for j in -n..=n {
        for i in -n..=n {
            let (x, y) = (i as f32 * step, j as f32 * step);
            let r = x.hypot(y);
            if r > radius {
                continue;
            }
            // 多角形の内側か: 中心角を 1 辺ぶんに折り畳み、辺までの距離と比べる
            let a = (y.atan2(x) - rotation).rem_euclid(sector) - sector / 2.0;
            if r * a.cos() <= radius * apothem_ratio + 1e-3 {
                taps.push((x, y));
            }
        }
    }
    taps
}

/// 標本点の重み付き平均。明るい点ほど重くして、玉ボケの形がはっきり出るようにする。
fn gather(buf: &FrameBuf, taps: &[(f32, f32)], gain: f32, threshold: f32) -> FrameBuf {
    generate(buf.width, buf.height, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut acc = [0.0f32; 4];
        let mut wsum = 0.0f32;
        for (dx, dy) in taps {
            let c = sample_clamp(buf, px + dx, py + dy);
            let w = 1.0 + gain * (display_luma(c) - threshold).max(0.0) * 8.0;
            for k in 0..4 {
                acc[k] += c[k] * w;
            }
            wsum += w;
        }
        acc.map(|v| v / wsum.max(1e-6))
    })
}

/// 絞り形状（正多角形）でぼかす。カメラの被写界深度が使う。
pub fn iris_blur(buf: &FrameBuf, radius: f32, sides: f32) -> FrameBuf {
    // 画像の外は透明として扱いたいので、端の延長ではなく透明サンプルで集める
    let taps = iris_taps(radius, sides, 0.0);
    generate(buf.width, buf.height, |x, y| {
        let mut acc = [0.0f32; 4];
        for (dx, dy) in &taps {
            let c = sample(buf, x as f32 + 0.5 + dx, y as f32 + 0.5 + dy);
            for k in 0..4 {
                acc[k] += c[k];
            }
        }
        acc.map(|v| v / taps.len().max(1) as f32)
    })
}

pub fn apply(kind: &str, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let s = ctx.scale;
    match kind {
        "gaussian_blur" => {
            let sigma = sigma_for(p.f("radius"), s);
            let (sx, sy) = match p.i("dimensions") {
                1 => (sigma, 0.0),
                2 => (0.0, sigma),
                _ => (sigma, sigma),
            };
            gaussian_buf(&mut buf, sx, sy, if p.b("repeat_edge") { Edge::Clamp } else { Edge::Zero });
            buf
        }
        "directional_blur" => {
            let len = p.f("length") as f32 * s;
            // 角度 0 は上方向（時計回り）
            let a = (p.f("angle") as f32 - 90.0).to_radians();
            let (dx, dy) = (a.cos() * len, a.sin() * len);
            let n = (len.ceil() as usize).clamp(1, 96);
            average_along(&buf, n, |x, y, u| (x + dx * (u - 0.5), y + dy * (u - 0.5)))
        }
        "radial_blur" => {
            let c = p.p("center");
            let (cx, cy) = (c[0] as f32 * s, c[1] as f32 * s);
            let amount = (p.f("amount") as f32).to_radians();
            let reach = (buf.width.max(buf.height) as f32) * amount;
            let n = (reach.ceil() as usize).clamp(1, 64);
            average_along(&buf, n, |x, y, u| {
                let (sn, cs) = (amount * (u - 0.5)).sin_cos();
                let (dx, dy) = (x - cx, y - cy);
                (cx + dx * cs - dy * sn, cy + dx * sn + dy * cs)
            })
        }
        "zoom_blur" => {
            let c = p.p("center");
            let (cx, cy) = (c[0] as f32 * s, c[1] as f32 * s);
            let amount = p.f("amount") as f32 / 100.0;
            let reach = (buf.width.max(buf.height) as f32) * amount;
            let n = (reach.ceil() as usize).clamp(1, 64);
            average_along(&buf, n, |x, y, u| {
                let k = 1.0 - amount * u;
                (cx + (x - cx) * k, cy + (y - cy) * k)
            })
        }
        "lens_blur" => {
            let radius = p.f("radius") as f32 * s;
            if radius < 0.5 {
                return buf;
            }
            let taps = iris_taps(radius, p.f("sides") as f32, (p.f("rotation") as f32).to_radians());
            gather(&buf, &taps, p.f("highlight_gain") as f32, p.f("highlight_threshold") as f32)
        }
        "compound_blur" => {
            let Some(map) = p.layer("map").and_then(|id| (ctx.fetch_layer)(id)) else { return buf };
            let max_sigma = sigma_for(p.f("max_radius"), s);
            let invert = p.b("invert");
            const LEVELS: usize = 5;
            let levels: Vec<FrameBuf> = (0..LEVELS)
                .map(|k| {
                    let mut b = buf.clone();
                    let sg = max_sigma * k as f32 / (LEVELS - 1) as f32;
                    gaussian_buf(&mut b, sg, sg, Edge::Clamp);
                    b
                })
                .collect();
            generate(buf.width, buf.height, |x, y| {
                let v = display_luma(map.pixel(x.min(map.width - 1), y.min(map.height - 1))).clamp(0.0, 1.0);
                let v = if invert { 1.0 - v } else { v };
                let pos = v * (LEVELS - 1) as f32;
                let k = (pos.floor() as usize).min(LEVELS - 2);
                mix4(levels[k].pixel(x, y), levels[k + 1].pixel(x, y), pos - k as f32)
            })
        }
        _ => buf,
    }
}
