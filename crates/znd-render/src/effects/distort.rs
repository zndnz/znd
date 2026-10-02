//! 歪み系。すべて逆写像（出力画素ごとに元画像のどこを見るか）で実装する。

use super::{fbm, remap, sample, to_display, FxContext};
use crate::color::luma;
use crate::frame::FrameBuf;
use znd_core::effect::EvalParams;

/// マップ画素から置き換え量（-1..1）を取り出す。中間グレー（表示値 0.5）が「置き換えなし」。
fn channel_value(px: [f32; 4], channel: u32) -> f32 {
    let d = to_display(px);
    let v = match channel {
        0 => d[0],
        1 => d[1],
        2 => d[2],
        3 => px[3],
        _ => luma(d),
    };
    // 透明な部分は置き換えなしとして扱う
    if px[3] <= 0.0 && channel != 3 {
        0.0
    } else {
        (v - 0.5) * 2.0
    }
}

fn wave(kind: u32, x: f32) -> f32 {
    let f = x.rem_euclid(1.0);
    match kind {
        1 => {
            if f < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        2 => 1.0 - 4.0 * (f - 0.5).abs(),
        3 => 2.0 * f - 1.0,
        _ => (f * std::f32::consts::TAU).sin(),
    }
}

/// 4x4 格子の変位を双一次補間する。
fn mesh_offset(offsets: &[[f64; 2]], u: f32, v: f32) -> (f32, f32) {
    if offsets.len() < 16 {
        return (0.0, 0.0);
    }
    let (gx, gy) = ((u * 3.0).clamp(0.0, 3.0), (v * 3.0).clamp(0.0, 3.0));
    let (ix, iy) = ((gx.floor() as usize).min(2), (gy.floor() as usize).min(2));
    let (fx, fy) = (gx - ix as f32, gy - iy as f32);
    let at = |x: usize, y: usize| offsets[y * 4 + x];
    let lerp = |a: [f64; 2], b: [f64; 2], t: f32| [a[0] as f32 + (b[0] - a[0]) as f32 * t, a[1] as f32 + (b[1] - a[1]) as f32 * t];
    let top = lerp(at(ix, iy), at(ix + 1, iy), fx);
    let bot = lerp(at(ix, iy + 1), at(ix + 1, iy + 1), fx);
    (top[0] + (bot[0] - top[0]) * fy, top[1] + (bot[1] - top[1]) * fy)
}

pub fn apply(kind: &str, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let s = ctx.scale;
    let (w, h) = (buf.width as f32, buf.height as f32);
    match kind {
        "displacement_map" => {
            let Some(map) = p.layer("map").and_then(|id| (ctx.fetch_layer)(id)) else { return buf };
            let (mh, mv) = (p.f("max_h") as f32 * s, p.f("max_v") as f32 * s);
            let (ch, cv) = (p.i("channel_h"), p.i("channel_v"));
            remap(&buf, |x, y| {
                let m = sample(&map, x, y);
                (x - channel_value(m, ch) * mh, y - channel_value(m, cv) * mv)
            })
        }
        "turbulent_displace" => {
            let amount = p.f("amount") as f32 * s;
            let size = (p.f("size") as f32 * s).max(1.0);
            let z = p.f("evolution") as f32 / 360.0;
            let oct = p.f("complexity") as f32;
            let seed = ctx.seed;
            remap(&buf, |x, y| {
                let dx = fbm(x / size, y / size, z, oct, false, seed) - 0.5;
                let dy = fbm(x / size + 31.7, y / size - 12.3, z, oct, false, seed ^ 0x5bd1_e995) - 0.5;
                (x + dx * 2.0 * amount, y + dy * 2.0 * amount)
            })
        }
        "wave_warp" => {
            let height = p.f("height") as f32 * s;
            let width = (p.f("width") as f32 * s).max(0.5);
            // 方向 90° で波は水平に進み、画素は垂直に揺れる
            let a = (p.f("direction") as f32 - 90.0).to_radians();
            let (dx, dy) = (a.cos(), a.sin());
            let phase = p.f("phase") as f32 / 360.0 + p.f("speed") as f32 * ctx.time.seconds() as f32;
            let kind = p.i("wave_type");
            remap(&buf, |x, y| {
                let along = (x * -dy + y * dx) / width;
                let d = wave(kind, along - phase) * height;
                (x - dx * d, y - dy * d)
            })
        }
        "polar_coordinates" => {
            let t = (p.f("interpolation") as f32 / 100.0).clamp(0.0, 1.0);
            let to_polar = p.i("mode") == 0;
            let (cx, cy) = (w / 2.0, h / 2.0);
            let max_r = cx.min(cy);
            remap(&buf, |x, y| {
                let (sx, sy) = if to_polar {
                    // 出力は極座標の見た目。角度 → 元画像の x、半径 → 元画像の y
                    let (dx, dy) = (x - cx, y - cy);
                    let ang = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
                    (ang * w, dx.hypot(dy) / max_r * h)
                } else {
                    let ang = x / w * std::f32::consts::TAU;
                    let r = y / h * max_r;
                    (cx + r * ang.sin(), cy - r * ang.cos())
                };
                (x + (sx - x) * t, y + (sy - y) * t)
            })
        }
        "mesh_warp" => {
            let amount = (p.f("amount") as f32 / 100.0) * s;
            let offsets = p.curve("offsets").to_vec();
            remap(&buf, |x, y| {
                let (ox, oy) = mesh_offset(&offsets, x / w, y / h);
                (x - ox * amount, y - oy * amount)
            })
        }
        "liquify" => {
            let amount = p.f("amount") as f32 / 100.0;
            let r = (p.f("radius") as f32 * s).max(1.0);
            let strokes: Vec<([f32; 2], [f32; 2])> = p
                .curve("strokes")
                .chunks_exact(2)
                .map(|c| ([c[0][0] as f32 * s, c[0][1] as f32 * s], [c[1][0] as f32 * s, c[1][1] as f32 * s]))
                .collect();
            if strokes.is_empty() {
                return buf;
            }
            remap(&buf, |x, y| {
                let (mut sx, mut sy) = (x, y);
                for (a, b) in &strokes {
                    // 押した先（b）の周りの画素が、押す前の位置（a 側）の絵を見る
                    let d2 = (x - b[0]).powi(2) + (y - b[1]).powi(2);
                    let k = (-d2 / (r * r)).exp() * amount;
                    sx -= (b[0] - a[0]) * k;
                    sy -= (b[1] - a[1]) * k;
                }
                (sx, sy)
            })
        }
        "mirror" => {
            let c = p.p("center");
            let (cx, cy) = (c[0] as f32 * s, c[1] as f32 * s);
            // 反射軸の法線方向。角度 0 で左側を右側へ映す
            let a = (p.f("angle") as f32).to_radians();
            let (nx, ny) = (a.cos(), a.sin());
            remap(&buf, |x, y| {
                let d = (x - cx) * nx + (y - cy) * ny;
                if d > 0.0 {
                    (x - 2.0 * d * nx, y - 2.0 * d * ny)
                } else {
                    (x, y)
                }
            })
        }
        "motion_tile" => {
            let c = p.p("center");
            let (cx, cy) = (c[0] as f32 * s, c[1] as f32 * s);
            let (tw, th) = (w * (p.f("tile_width") as f32 / 100.0).max(0.01), h * (p.f("tile_height") as f32 / 100.0).max(0.01));
            let mirror = p.b("mirror_edges");
            let phase = p.f("phase") as f32 / 360.0;
            super::generate(buf.width, buf.height, |xi, yi| {
                let (x, y) = (xi as f32 + 0.5, yi as f32 + 0.5);
                let u = (x - cx) / tw + 0.5;
                let col = u.floor();
                // フェーズは 1 列おきに縦へずらす
                let v = (y - cy) / th + 0.5 + if (col as i64).rem_euclid(2) == 1 { phase } else { 0.0 };
                let row = v.floor();
                let (mut fu, mut fv) = (u - col, v - row);
                if mirror {
                    if (col as i64).rem_euclid(2) == 1 {
                        fu = 1.0 - fu;
                    }
                    if (row as i64).rem_euclid(2) == 1 {
                        fv = 1.0 - fv;
                    }
                }
                // タイルの継ぎ目は元画像の端どうしなので、クランプしたサンプルでつなぐ
                super::sample_clamp(&buf, fu * w, fv * h)
            })
        }
        _ => buf,
    }
}
