//! キーイング: クロマキー（スピル除去付き）、ルミナンスキー、マットチョーク。

use super::blur::{gaussian_buf, sigma_for};
use super::{display_luma, from_display, map_pixels, smoothstep, to_display, FxContext};
use crate::blur::Edge;
use crate::frame::FrameBuf;
use znd_core::effect::EvalParams;

/// 表示値 RGB → 色差（Cb, Cr）。明るさの違いに影響されにくい距離を測るため。
fn chroma(c: [f32; 3]) -> (f32, f32) {
    let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    ((c[2] - y) / 1.8556, (c[0] - y) / 1.5748)
}

pub fn apply(kind: &str, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    match kind {
        "chroma_key" => {
            let key = p.c("key_color");
            let key = [key[0] as f32, key[1] as f32, key[2] as f32];
            let (kcb, kcr) = chroma(key);
            let tol = p.f("tolerance") as f32 * 0.5;
            let soft = (p.f("softness") as f32 * 0.5).max(1e-4);
            let despill = (p.f("despill") as f32 / 100.0).clamp(0.0, 1.0);
            // キーカラーで最も強いチャンネル（緑バックなら G）
            let dom = if key[1] >= key[0] && key[1] >= key[2] {
                1
            } else if key[2] >= key[0] {
                2
            } else {
                0
            };
            let (o1, o2) = ((dom + 1) % 3, (dom + 2) % 3);
            map_pixels(&mut buf, |_, _, px| {
                if px[3] <= 0.0 {
                    return px;
                }
                let mut d = to_display(px);
                let (cb, cr) = chroma(d);
                let dist = (cb - kcb).hypot(cr - kcr);
                let alpha = smoothstep(tol, tol + soft, dist);
                // スピル除去: キー色のチャンネルが他の 2 つの平均を超えないようにする
                let limit = (d[o1] + d[o2]) / 2.0;
                if d[dom] > limit {
                    d[dom] += (limit - d[dom]) * despill;
                }
                from_display(d, px[3] * alpha)
            });
            buf
        }
        "luma_key" => {
            let th = p.f("threshold") as f32;
            let soft = (p.f("softness") as f32).max(1e-4);
            let key_bright = p.i("mode") == 0;
            map_pixels(&mut buf, |_, _, px| {
                let l = display_luma(px);
                let k = if key_bright { 1.0 - smoothstep(th - soft, th + soft, l) } else { smoothstep(th - soft, th + soft, l) };
                px.map(|v| v * k)
            });
            buf
        }
        "matte_choker" => {
            let choke = p.f("choke");
            let blur = sigma_for(p.f("blur"), ctx.scale);
            let mut alpha = buf.clone();
            if choke != 0.0 {
                // アルファをぼかしてしきい値をずらすと、輪郭が内側（正）／外側（負）へ動く
                let sg = sigma_for(choke.abs() * 2.0, ctx.scale);
                gaussian_buf(&mut alpha, sg, sg, Edge::Zero);
                let th = if choke > 0.0 { 0.85 } else { 0.15 };
                map_pixels(&mut alpha, |_, _, a| [0.0, 0.0, 0.0, smoothstep(th - 0.1, th + 0.1, a[3])]);
            }
            if blur > 0.0 {
                gaussian_buf(&mut alpha, blur, blur, Edge::Zero);
            }
            map_pixels(&mut buf, |x, y, px| {
                if px[3] <= 0.0 {
                    return px;
                }
                let k = (alpha.pixel(x, y)[3] / px[3]).min(if choke < 0.0 { 1.0 / px[3] } else { 1.0 });
                px.map(|v| v * k)
            });
            buf
        }
        _ => buf,
    }
}
