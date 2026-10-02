//! 時間系: エコー、残像、ピクセルモーションブラー、時間置き換え。
//!
//! 別時刻の絵は `FxContext::fetch_self`（このレイヤーのエフェクト適用前の画像）から得る。

use super::{display_luma, generate, mix4, sample, FxContext};
use crate::frame::FrameBuf;
use znd_core::effect::EvalParams;
use znd_core::Time;

fn offset(t: Time, seconds: f64) -> Time {
    t + Time::from_seconds(seconds)
}

fn echo(buf: FrameBuf, ctx: &FxContext, step: f64, count: usize, start: f32, decay: f32, operator: u32) -> FrameBuf {
    let mut out = buf;
    if operator != 4 {
        out.data.iter_mut().for_each(|v| *v *= start);
    }
    let mut intensity = start;
    for i in 1..=count {
        intensity *= decay;
        let Some(e) = (ctx.fetch_self)(offset(ctx.time, step * i as f64)) else { continue };
        if e.width != out.width || e.height != out.height {
            continue;
        }
        for (d, s) in out.data.chunks_exact_mut(4).zip(e.data.chunks_exact(4)) {
            let s = [s[0] * intensity, s[1] * intensity, s[2] * intensity, s[3] * intensity];
            match operator {
                // 最大
                1 => (0..4).for_each(|k| d[k] = d[k].max(s[k])),
                // スクリーン
                2 => (0..4).for_each(|k| d[k] = d[k] + s[k] - d[k] * s[k]),
                // 後ろに合成（エコーを今の絵の後ろへ）
                3 => {
                    let k = 1.0 - d[3];
                    (0..4).for_each(|j| d[j] += s[j] * k);
                }
                // 前に合成
                4 => {
                    let k = 1.0 - s[3];
                    (0..4).for_each(|j| d[j] = s[j] + d[j] * k);
                }
                // 加算
                _ => {
                    (0..3).for_each(|k| d[k] += s[k]);
                    d[3] = (d[3] + s[3]).min(1.0);
                }
            }
        }
    }
    out
}

/// 粗いブロックマッチングで前フレームからの動きベクトル場（ブロック単位、px）を推定する。
fn block_flow(prev: &FrameBuf, cur: &FrameBuf, block: usize, range: i32) -> (usize, usize, Vec<(f32, f32)>) {
    let (w, h) = (cur.width, cur.height);
    let (bw, bh) = (w.div_ceil(block), h.div_ceil(block));
    let lum = |b: &FrameBuf| -> Vec<f32> { b.data.chunks_exact(4).map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).collect() };
    let (lp, lc) = (lum(prev), lum(cur));
    let mut flow = vec![(0.0f32, 0.0f32); bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let (x0, y0) = (bx * block, by * block);
            let mut best = (f32::MAX, 0i32, 0i32);
            for dy in (-range..=range).step_by(2) {
                for dx in (-range..=range).step_by(2) {
                    let mut sad = 0.0f32;
                    for y in (y0..(y0 + block).min(h)).step_by(2) {
                        for x in (x0..(x0 + block).min(w)).step_by(2) {
                            let (px, py) = (x as i32 - dx, y as i32 - dy);
                            let prev_v =
                                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 { 0.0 } else { lp[py as usize * w + px as usize] };
                            sad += (lc[y * w + x] - prev_v).abs();
                        }
                    }
                    // 同点なら動きの小さい方を選ぶ（平坦な領域で暴れない）
                    let penalty = 1e-4 * (dx.abs() + dy.abs()) as f32;
                    if sad + penalty < best.0 {
                        best = (sad + penalty, dx, dy);
                    }
                }
            }
            flow[by * bw + bx] = (best.1 as f32, best.2 as f32);
        }
    }
    (bw, bh, flow)
}

pub fn apply(kind: &str, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    match kind {
        "echo" => echo(
            buf,
            ctx,
            p.f("echo_time"),
            p.f("count").round().clamp(0.0, 64.0) as usize,
            p.f("starting_intensity") as f32,
            p.f("decay") as f32,
            p.i("operator"),
        ),
        "afterimage" => {
            let frame = ctx.frame_rate.den as f64 / ctx.frame_rate.num as f64;
            echo(buf, ctx, -frame, p.f("frames").round().clamp(1.0, 64.0) as usize, 1.0, p.f("decay") as f32, 3)
        }
        "pixel_motion_blur" => {
            let frame = ctx.frame_rate.den as f64 / ctx.frame_rate.num as f64;
            let Some(prev) = (ctx.fetch_self)(offset(ctx.time, -frame)) else { return buf };
            if prev.width != buf.width || prev.height != buf.height {
                return buf;
            }
            let block = 16usize;
            let (bw, bh, flow) = block_flow(&prev, &buf, block, 24);
            let shutter = (p.f("shutter_angle") / 360.0) as f32;
            let n = p.f("samples").round().clamp(2.0, 32.0) as usize;
            generate(buf.width, buf.height, |x, y| {
                // ブロック中心間で動きベクトルを双一次補間する
                let (gx, gy) = ((x as f32 + 0.5) / block as f32 - 0.5, (y as f32 + 0.5) / block as f32 - 0.5);
                let (ix, iy) = (gx.floor().clamp(0.0, bw as f32 - 1.0) as usize, gy.floor().clamp(0.0, bh as f32 - 1.0) as usize);
                let (jx, jy) = ((ix + 1).min(bw - 1), (iy + 1).min(bh - 1));
                let (fx, fy) = ((gx - ix as f32).clamp(0.0, 1.0), (gy - iy as f32).clamp(0.0, 1.0));
                let f = |a: usize, b: usize| flow[b * bw + a];
                let lerp = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let v = lerp(lerp(f(ix, iy), f(jx, iy), fx), lerp(f(ix, jy), f(jx, jy), fx), fy);
                let mut acc = [0.0f32; 4];
                for i in 0..n {
                    let u = (i as f32 / (n - 1) as f32 - 0.5) * shutter;
                    let s = sample(&buf, x as f32 + 0.5 + v.0 * u, y as f32 + 0.5 + v.1 * u);
                    (0..4).for_each(|k| acc[k] += s[k]);
                }
                acc.map(|a| a / n as f32)
            })
        }
        "time_displacement" => {
            let Some(map) = p.layer("map").and_then(|id| (ctx.fetch_layer)(id)) else { return buf };
            let max = p.f("max_displacement");
            let steps = p.f("steps").round().clamp(2.0, 60.0) as usize;
            // マップの明るさを段階に量子化し、段階ごとの時刻の絵を用意する
            let frames: Vec<Option<FrameBuf>> = (0..steps)
                .map(|k| {
                    let v = k as f64 / (steps - 1) as f64;
                    let dt = (v - 0.5) * 2.0 * max;
                    if dt.abs() < 1e-9 {
                        Some(buf.clone())
                    } else {
                        (ctx.fetch_self)(offset(ctx.time, dt)).filter(|f| f.width == buf.width && f.height == buf.height)
                    }
                })
                .collect();
            generate(buf.width, buf.height, |x, y| {
                let v = display_luma(map.pixel(x.min(map.width - 1), y.min(map.height - 1))).clamp(0.0, 1.0);
                let pos = v * (steps - 1) as f32;
                let k = (pos.floor() as usize).min(steps - 2);
                let get = |i: usize| frames[i].as_ref().map(|f| f.pixel(x, y)).unwrap_or([0.0; 4]);
                mix4(get(k), get(k + 1), pos - k as f32)
            })
        }
        _ => buf,
    }
}
