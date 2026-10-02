//! 光系: グロー、ブルーム、レンズフレア、光線、ドロップシャドウ。

use super::blur::{gaussian_buf, sigma_for};
use super::{display_luma, generate, map_pixels, sample, smoothstep, FxContext};
use crate::blur::Edge;
use crate::frame::FrameBuf;
use znd_core::effect::EvalParams;

/// しきい値より明るい部分だけを取り出す（しきい値付近は滑らかに立ち上げる）。
fn bright_pass(buf: &FrameBuf, threshold: f32) -> FrameBuf {
    let mut out = buf.clone();
    map_pixels(&mut out, |_, _, px| {
        let k = smoothstep(threshold - 0.1, threshold + 0.1, display_luma(px));
        px.map(|v| v * k)
    });
    out
}

/// `glow` を元の画像に加算する。アルファは光った部分だけ持ち上げる。
fn add_light(mut base: FrameBuf, glow: &FrameBuf, intensity: f32, tint: [f32; 3]) -> FrameBuf {
    for (b, g) in base.data.chunks_exact_mut(4).zip(glow.data.chunks_exact(4)) {
        let add = [g[0] * tint[0] * intensity, g[1] * tint[1] * intensity, g[2] * tint[2] * intensity];
        b[0] += add[0];
        b[1] += add[1];
        b[2] += add[2];
        let ga = (g[3] * intensity).clamp(0.0, 1.0);
        b[3] = b[3] + ga - b[3] * ga;
    }
    base
}

/// 半径を倍々にした `layers` 段のぼかしを重ねる（芯が明るく、裾が長い光になる）。
fn multi_blur(src: &FrameBuf, radius: f64, layers: usize, scale: f32) -> FrameBuf {
    let layers = layers.clamp(1, 8);
    let mut acc = FrameBuf::new(src.width, src.height);
    for i in 0..layers {
        let r = radius / 2f64.powi((layers - 1 - i) as i32);
        let mut b = src.clone();
        let sg = sigma_for(r, scale);
        gaussian_buf(&mut b, sg, sg, Edge::Zero);
        acc.data.iter_mut().zip(&b.data).for_each(|(a, v)| *a += v / layers as f32);
    }
    acc
}

pub fn apply(kind: &str, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let s = ctx.scale;
    match kind {
        "glow" | "bloom" => {
            let bright = bright_pass(&buf, p.f("threshold") as f32);
            let layers = if kind == "glow" { p.f("layers").round() as usize } else { 3 };
            let glow = multi_blur(&bright, p.f("radius"), layers, s);
            let tint = if kind == "glow" {
                let c = ctx.cx.premul(p.c("color"));
                [c[0], c[1], c[2]]
            } else {
                [1.0; 3]
            };
            add_light(buf, &glow, p.f("intensity") as f32, tint)
        }
        "lens_flare" => {
            let c = p.p("center");
            let (cx, cy) = (c[0] as f32 * s, c[1] as f32 * s);
            let (w, h) = (buf.width as f32, buf.height as f32);
            let (mx, my) = (w / 2.0, h / 2.0);
            let size = w.max(h);
            let col = ctx.cx.premul(p.c("color"));
            let k = p.f("brightness") as f32 / 100.0;
            // 光源から画面中心を通る軸上に並ぶゴースト: (軸上の位置, 半径比, 強さ)
            const GHOSTS: [(f32, f32, f32); 5] =
                [(0.5, 0.03, 0.25), (0.9, 0.06, 0.12), (1.3, 0.02, 0.3), (1.6, 0.1, 0.08), (2.0, 0.05, 0.15)];
            let flare = generate(buf.width, buf.height, |x, y| {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = (px - cx).hypot(py - cy) / size;
                // 中心の光芒、輪、横に伸びる筋
                let mut v = (-d * d * 900.0).exp() + 0.35 * (-d * 14.0).exp();
                v += 0.12 * (-((d - 0.18) * 60.0).powi(2)).exp();
                v += 0.3 * (-((py - cy) / (size * 0.004)).powi(2)).exp() * (-((px - cx) / (size * 0.35)).powi(2)).exp();
                for (t, r, a) in GHOSTS {
                    let (gx, gy) = (cx + (mx - cx) * t, cy + (my - cy) * t);
                    let gd = (px - gx).hypot(py - gy) / (size * r);
                    v += a * (1.0 - smoothstep(0.7, 1.0, gd));
                }
                let v = v * k;
                [col[0] * v, col[1] * v, col[2] * v, v.clamp(0.0, 1.0)]
            });
            add_light(buf, &flare, 1.0, [1.0; 3])
        }
        "light_rays" => {
            let c = p.p("center");
            let (cx, cy) = (c[0] as f32 * s, c[1] as f32 * s);
            let bright = bright_pass(&buf, p.f("threshold") as f32);
            let length = (p.f("length") as f32 / 100.0).clamp(0.0, 0.99);
            const STEPS: usize = 48;
            // 光源へ向かって明るい部分を引き伸ばす
            let rays = generate(buf.width, buf.height, |x, y| {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut acc = [0.0f32; 4];
                for i in 0..STEPS {
                    let u = i as f32 / (STEPS - 1) as f32;
                    let kk = 1.0 - length * u;
                    let smp = sample(&bright, cx + (px - cx) * kk, cy + (py - cy) * kk);
                    let w = 1.0 - u * 0.7;
                    for j in 0..4 {
                        acc[j] += smp[j] * w;
                    }
                }
                acc.map(|v| v / STEPS as f32 * 1.5)
            });
            let col = ctx.cx.premul(p.c("color"));
            add_light(buf, &rays, p.f("intensity") as f32, [col[0], col[1], col[2]])
        }
        "drop_shadow" => {
            let a = (p.f("angle") as f32 - 90.0).to_radians();
            let dist = p.f("distance") as f32 * s;
            let (dx, dy) = (a.cos() * dist, a.sin() * dist);
            let col = ctx.cx.premul(p.c("color"));
            let opacity = (p.f("opacity") as f32 / 100.0).clamp(0.0, 1.0);
            let mut shadow = generate(buf.width, buf.height, |x, y| {
                let al = sample(&buf, x as f32 + 0.5 - dx, y as f32 + 0.5 - dy)[3] * opacity;
                [col[0] * al, col[1] * al, col[2] * al, col[3] * al]
            });
            let sg = sigma_for(p.f("softness"), s);
            gaussian_buf(&mut shadow, sg, sg, Edge::Zero);
            // 元の画像を影の上に重ねる
            for (sh, o) in shadow.data.chunks_exact_mut(4).zip(buf.data.chunks_exact(4)) {
                let k = 1.0 - o[3];
                for j in 0..4 {
                    sh[j] = o[j] + sh[j] * k;
                }
            }
            shadow
        }
        _ => buf,
    }
}
