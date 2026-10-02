//! スタイライズ系。

use super::{
    blend_with_original, display_luma, from_display, generate, map_display, map_pixels, noise3, rand01, sample, sample_clamp, to_display,
    FxContext,
};
use crate::frame::FrameBuf;
use znd_core::effect::EvalParams;

/// 現在のフレーム番号（フレームごとに変わる乱数の種）。
fn frame_index(ctx: &FxContext) -> u32 {
    ctx.frame_rate.time_to_frame(ctx.time) as u32
}

pub fn apply(kind: &str, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let s = ctx.scale;
    let (w, h) = (buf.width, buf.height);
    match kind {
        "glitch" => {
            let amount = (p.f("amount") as f32 / 100.0).clamp(0.0, 1.0);
            let block = (p.f("block_size") as f32 * s).max(1.0);
            let shift = p.f("rgb_shift") as f32 * s;
            // 「速度」回／秒で乱れ方が切り替わる
            let tick = (ctx.time.seconds() * p.f("speed")).floor() as i64 as u32;
            let seed = p.f("seed") as u32 ^ ctx.seed;
            generate(w, h, |x, y| {
                let band = (y as f32 / block) as u32;
                // 帯ごとに、確率 amount で横にずれる
                let active = rand01(band, tick, seed) < amount;
                let dx = if active { (rand01(band, tick, seed ^ 1) - 0.5) * 2.0 * amount * w as f32 * 0.25 } else { 0.0 };
                let sh = if active { shift * (0.5 + rand01(band, tick, seed ^ 2)) } else { shift * amount * 0.25 };
                let (px, py) = (x as f32 + 0.5 + dx, y as f32 + 0.5);
                let (r, g, b) = (sample_clamp(&buf, px + sh, py), sample_clamp(&buf, px, py), sample_clamp(&buf, px - sh, py));
                [r[0], g[1], b[2], (r[3] + g[3] + b[3]) / 3.0]
            })
        }
        "rgb_split" => {
            let amount = p.f("amount") as f32 * s;
            let a = (p.f("angle") as f32).to_radians();
            let radial = p.b("radial");
            let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
            let reach = cx.hypot(cy).max(1.0);
            generate(w, h, |x, y| {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                // 放射状: 中心から離れるほど大きくずれる（レンズの色収差）
                let (dx, dy) =
                    if radial { ((px - cx) / reach * amount, (py - cy) / reach * amount) } else { (a.cos() * amount, a.sin() * amount) };
                let (r, g, b) = (sample(&buf, px + dx, py + dy), sample(&buf, px, py), sample(&buf, px - dx, py - dy));
                [r[0], g[1], b[2], (r[3] + g[3] + b[3]) / 3.0]
            })
        }
        "posterize" => {
            let n = (p.f("levels") as f32).clamp(2.0, 255.0) - 1.0;
            map_display(&mut buf, |c| c.map(|v| (v.clamp(0.0, 1.0) * n).round() / n));
            buf
        }
        "halftone" => {
            let size = (p.f("dot_size") as f32 * s).max(1.5);
            let base = (p.f("angle") as f32).to_radians();
            let color = p.b("color");
            // カラー時はチャンネルごとに網点の角度を変える（モアレを避ける印刷の流儀）
            let angles = if color { [base + 0.26, base + 1.31, base] } else { [base; 3] };
            generate(w, h, |x, y| {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut out = [0.0f32; 3];
                let mut alpha = 0.0f32;
                for (ch, ang) in angles.iter().enumerate() {
                    let (sn, cs) = ang.sin_cos();
                    // 回転した格子でのセル中心を求め、その位置の明るさで点の大きさを決める
                    let (u, v) = ((px * cs + py * sn) / size, (-px * sn + py * cs) / size);
                    let (cu, cv) = (u.floor() + 0.5, v.floor() + 0.5);
                    let (wx, wy) = ((cu * cs - cv * sn) * size, (cu * sn + cv * cs) * size);
                    let smp = sample_clamp(&buf, wx, wy);
                    let d = to_display(smp);
                    let val = if color { d[ch] } else { super::display_luma(smp) };
                    let radius = val.clamp(0.0, 1.0).sqrt() * size * 0.72;
                    let dist = ((u - cu) * size).hypot((v - cv) * size);
                    out[ch] = (radius - dist + 0.5).clamp(0.0, 1.0);
                    alpha = alpha.max(smp[3]);
                }
                from_display(out, alpha)
            })
        }
        "mosaic" => {
            let (nx, ny) = (p.f("h_blocks").max(1.0) as f32, p.f("v_blocks").max(1.0) as f32);
            let (bw, bh) = (w as f32 / nx, h as f32 / ny);
            // ブロックごとの平均色を先に求める
            let (cols, rows) = (nx.ceil() as usize, ny.ceil() as usize);
            let mut sums = vec![[0.0f32; 5]; cols * rows];
            for y in 0..h {
                for x in 0..w {
                    let i = ((y as f32 / bh) as usize).min(rows - 1) * cols + ((x as f32 / bw) as usize).min(cols - 1);
                    let px = buf.pixel(x, y);
                    (0..4).for_each(|k| sums[i][k] += px[k]);
                    sums[i][4] += 1.0;
                }
            }
            map_pixels(&mut buf, |x, y, _| {
                let c = sums[((y as f32 / bh) as usize).min(rows - 1) * cols + ((x as f32 / bw) as usize).min(cols - 1)];
                let n = c[4].max(1.0);
                [c[0] / n, c[1] / n, c[2] / n, c[3] / n]
            });
            buf
        }
        "find_edges" => {
            let invert = p.b("invert");
            let lum =
                |x: isize, y: isize| display_luma(buf.pixel(x.clamp(0, w as isize - 1) as usize, y.clamp(0, h as isize - 1) as usize));
            let edges = generate(w, h, |x, y| {
                let (x, y) = (x as isize, y as isize);
                // ソーベル
                let gx = lum(x + 1, y - 1) + 2.0 * lum(x + 1, y) + lum(x + 1, y + 1)
                    - lum(x - 1, y - 1)
                    - 2.0 * lum(x - 1, y)
                    - lum(x - 1, y + 1);
                let gy = lum(x - 1, y + 1) + 2.0 * lum(x, y + 1) + lum(x + 1, y + 1)
                    - lum(x - 1, y - 1)
                    - 2.0 * lum(x, y - 1)
                    - lum(x + 1, y - 1);
                let e = gx.hypot(gy).min(1.0);
                // 既定は白地に黒い輪郭、反転で黒地に白い輪郭
                let v = if invert { e } else { 1.0 - e };
                from_display([v; 3], buf.pixel(x as usize, y as usize)[3])
            });
            blend_with_original(&buf, edges, p.f("blend"))
        }
        "noise" => {
            let amount = p.f("amount") as f32 / 100.0;
            let color = p.b("color_noise");
            let f = if p.b("animated") { frame_index(ctx) } else { 0 };
            let seed = ctx.seed;
            map_pixels(&mut buf, |x, y, px| {
                if px[3] <= 0.0 {
                    return px;
                }
                let d = to_display(px);
                let n = |c: u32| (rand01(x as u32, y as u32, f.wrapping_mul(31).wrapping_add(c) ^ seed) - 0.5) * 2.0 * amount;
                let m = n(0);
                let out = if color { [d[0] + n(0), d[1] + n(1), d[2] + n(2)] } else { [d[0] + m, d[1] + m, d[2] + m] };
                from_display(out.map(|v| v.max(0.0)), px[3])
            });
            buf
        }
        "film_grain" => {
            let amount = p.f("amount") as f32 / 100.0;
            let size = (p.f("size") as f32 * s).max(0.5);
            let f = frame_index(ctx) as f32;
            let seed = ctx.seed;
            map_pixels(&mut buf, |x, y, px| {
                if px[3] <= 0.0 {
                    return px;
                }
                let d = to_display(px);
                let g = (noise3(x as f32 / size, y as f32 / size, f * 7.31, seed) - 0.5) * 2.0;
                // 粒子は中間調で最も目立ち、純黒・純白では消える
                let l = super::luma(d).clamp(0.0, 1.0);
                let k = g * amount * 4.0 * l * (1.0 - l);
                from_display([d[0] + k, d[1] + k, d[2] + k].map(|v| v.max(0.0)), px[3])
            });
            buf
        }
        "scanlines" => {
            let spacing = (p.f("spacing") as f32 * s).max(1.0);
            let dark = (p.f("darkness") as f32 / 100.0).clamp(0.0, 1.0);
            let scroll = (p.f("speed") * ctx.time.seconds()) as f32 * s;
            map_pixels(&mut buf, |_, y, px| {
                let phase = ((y as f32 + 0.5 + scroll) / spacing).rem_euclid(1.0);
                // 1 周期のうち半分を暗くする（境界は 1 画素ぶんぼかす）
                let edge = (0.5 / spacing).min(0.25);
                let line = super::smoothstep(0.5 - edge, 0.5 + edge, phase);
                let k = 1.0 - dark * line;
                [px[0] * k, px[1] * k, px[2] * k, px[3]]
            });
            buf
        }
        _ => buf,
    }
}
