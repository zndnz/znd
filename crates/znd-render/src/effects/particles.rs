//! パーティクル。各粒子の位置は「生まれた時刻と乱数の種」から閉じた式で求めるので、
//! どのフレームからレンダリングしても同じ結果になり、フレームを並列に計算できる。

use super::{hash3, noise3, sample, FxContext};
use crate::frame::FrameBuf;
use znd_core::effect::EvalParams;
use znd_core::Time;

fn r01(i: u32, k: u32, seed: u32) -> f32 {
    (hash3(i, k, seed) >> 8) as f32 / (1u32 << 24) as f32
}

/// 重力下の放物運動に床との反発を入れた、経過時間 `age` 後の y 座標。
fn bounce_y(y0: f32, vy0: f32, g: f32, floor: f32, restitution: f32, age: f32) -> f32 {
    let (mut y, mut vy, mut t) = (y0, vy0, age);
    for _ in 0..16 {
        // 現在の放物線が床に届く時刻を解く: y + vy·τ + g/2·τ² = floor
        let hit = if g.abs() < 1e-6 {
            if vy > 0.0 {
                (floor - y) / vy
            } else {
                f32::MAX
            }
        } else {
            let disc = vy * vy - 2.0 * g * (y - floor);
            if disc < 0.0 {
                f32::MAX
            } else {
                let tau = (-vy + disc.sqrt()) / g;
                if tau > 1e-5 {
                    tau
                } else {
                    f32::MAX
                }
            }
        };
        if hit >= t {
            return (y + vy * t + 0.5 * g * t * t).min(floor);
        }
        t -= hit;
        vy = -(vy + g * hit) * restitution;
        y = floor;
        if vy.abs() < 1.0 {
            return floor;
        }
    }
    floor
}

pub fn apply(p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let s = ctx.scale;
    let (w, h) = (buf.width, buf.height);
    let mut out = FrameBuf::new(w, h);
    let t = ctx.time.seconds();
    let rate = p.f("rate");
    let life = p.f("life").max(1e-3);
    if rate <= 0.0 {
        return out;
    }
    let seed = p.f("seed") as u32 ^ ctx.seed;
    let emitter = p.i("emitter");
    let (ew, eh) = (p.f("emitter_width") as f32, p.f("emitter_height") as f32);
    let emitter_img = if emitter == 2 { p.layer("emitter_layer").and_then(|id| (ctx.fetch_layer)(id)) } else { None };
    let sprite = p.layer("sprite_layer").and_then(|id| (ctx.fetch_layer)(id));
    let (speed, speed_rand) = (p.f("speed") as f32, p.f("speed_random") as f32 / 100.0);
    let (dir, spread) = (p.f("direction") as f32, p.f("spread") as f32);
    let (gravity, turb) = (p.f("gravity") as f32, p.f("turbulence") as f32);
    let floor = p.b("floor_enabled").then(|| p.f("floor_y") as f32);
    let bounce = p.f("bounce") as f32;
    let (size0, size1) = (p.f("size") as f32, p.f("size") as f32 * p.f("size_end") as f32 / 100.0);
    let (c0, c1) = (ctx.cx.premul(p.c("color_start")), ctx.cx.premul(p.c("color_end")));
    let op1 = p.f("opacity_end") as f32 / 100.0;
    let trail = p.f("trail").round().clamp(0.0, 64.0) as usize;
    let additive = p.b("additive");

    // エミッター位置がアニメートされていても軌跡が残るよう、粒子は「生まれた時刻の位置」から出る。
    // 毎粒子パラメータを評価すると重いので、寿命の範囲を等間隔に標本化して補間する。
    const SAMPLES: usize = 48;
    let positions: Vec<[f64; 2]> = (0..=SAMPLES)
        .map(|k| {
            let bt = t - life + life * k as f64 / SAMPLES as f64;
            (ctx.eval_at)(Time::from_seconds(bt.max(0.0))).p("position")
        })
        .collect();
    let emitter_pos = |birth: f64| -> [f32; 2] {
        let u = ((birth - (t - life)) / life).clamp(0.0, 1.0) * SAMPLES as f64;
        let k = (u.floor() as usize).min(SAMPLES - 1);
        let f = u - k as f64;
        [
            (positions[k][0] + (positions[k + 1][0] - positions[k][0]) * f) as f32,
            (positions[k][1] + (positions[k + 1][1] - positions[k][1]) * f) as f32,
        ]
    };

    let first = ((t - life) * rate).ceil().max(0.0) as i64;
    let last = (t * rate).floor() as i64;
    for i in first..=last {
        let id = i as u32;
        let birth = i as f64 / rate;
        let age_now = (t - birth) as f32;
        let base = emitter_pos(birth);
        // 発生位置
        let p0 = match emitter {
            1 => [base[0] + (r01(id, 1, seed) - 0.5) * ew, base[1] + (r01(id, 2, seed) - 0.5) * eh],
            2 => {
                let Some(img) = &emitter_img else { continue };
                // レイヤーの不透明な場所から出す（決定的な棄却サンプリング）
                let mut found = None;
                for k in 0..12 {
                    let (x, y) = (r01(id, 10 + k * 2, seed) * w as f32, r01(id, 11 + k * 2, seed) * h as f32);
                    if sample(img, x, y)[3] > 0.5 {
                        found = Some([x / s, y / s]);
                        break;
                    }
                }
                match found {
                    Some(q) => q,
                    None => continue,
                }
            }
            _ => base,
        };
        let ang = (dir + (r01(id, 3, seed) - 0.5) * spread).to_radians();
        let sp = speed * (1.0 - r01(id, 4, seed) * speed_rand);
        let (vx, vy) = (ang.cos() * sp, ang.sin() * sp);

        for k in 0..=trail {
            // トレイル: 少し前の自分を、薄くして描く
            let age = age_now - k as f32 * 0.02;
            if age < 0.0 {
                break;
            }
            let u = (age / life as f32).clamp(0.0, 1.0);
            let mut x = p0[0] + vx * age;
            let mut y = match floor {
                Some(f) if p0[1] <= f => bounce_y(p0[1], vy, gravity, f, bounce, age),
                _ => p0[1] + vy * age + 0.5 * gravity * age * age,
            };
            if turb > 0.0 {
                x += (noise3(p0[0] * 0.01, age * 0.8, id as f32 * 0.37, seed) - 0.5) * 2.0 * turb * age;
                y += (noise3(p0[1] * 0.01 + 19.0, age * 0.8, id as f32 * 0.37, seed ^ 77) - 0.5) * 2.0 * turb * age;
            }
            let size = (size0 + (size1 - size0) * u).max(0.0) * s;
            let fade = (1.0 + (op1 - 1.0) * u) * (1.0 - k as f32 / (trail + 1) as f32);
            if size <= 0.01 || fade <= 0.0 {
                continue;
            }
            let col = [c0[0] + (c1[0] - c0[0]) * u, c0[1] + (c1[1] - c0[1]) * u, c0[2] + (c1[2] - c0[2]) * u, c0[3] + (c1[3] - c0[3]) * u];
            let (cx, cy, r) = (x * s, y * s, size / 2.0);
            let (x0, x1) = (((cx - r - 1.0).floor().max(0.0)) as usize, ((cx + r + 1.0).ceil().max(0.0) as usize).min(w));
            let (y0, y1) = (((cy - r - 1.0).floor().max(0.0)) as usize, ((cy + r + 1.0).ceil().max(0.0) as usize).min(h));
            for py in y0..y1 {
                for px in x0..x1 {
                    let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                    let src = match &sprite {
                        // スプライト: 参照レイヤーの絵を粒子の大きさに縮めて貼る
                        Some(img) => {
                            let (su, sv) = ((dx / (2.0 * r) + 0.5) * img.width as f32, (dy / (2.0 * r) + 0.5) * img.height as f32);
                            if dx.abs() > r || dy.abs() > r {
                                continue;
                            }
                            sample(img, su, sv).map(|v| v * fade)
                        }
                        None => {
                            // 1 画素幅の傾斜で縁をぼかすと面積が π/12 だけ増えるので、総量が円の面積に合うよう補正する
                            let cov = (r - dx.hypot(dy) + 0.5).clamp(0.0, 1.0) * fade * (r * r / (r * r + 1.0 / 12.0));
                            col.map(|v| v * cov)
                        }
                    };
                    if src[3] <= 0.0 && !additive {
                        continue;
                    }
                    let idx = (py * w + px) * 4;
                    let d = &mut out.data[idx..idx + 4];
                    if additive {
                        d[0] += src[0];
                        d[1] += src[1];
                        d[2] += src[2];
                        d[3] = (d[3] + src[3]).min(1.0);
                    } else {
                        let kk = 1.0 - src[3];
                        (0..4).for_each(|j| d[j] = src[j] + d[j] * kk);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::bounce_y;

    #[test]
    fn bounce_matches_free_fall_before_impact_and_stays_above_floor() {
        // 床に届く前は放物線そのもの
        let y = bounce_y(0.0, 0.0, 100.0, 1000.0, 0.5, 1.0);
        assert!((y - 50.0).abs() < 1e-3);
        // 落下 → 反発。床（y=50）より下には行かない
        for i in 0..200 {
            let y = bounce_y(0.0, 0.0, 100.0, 50.0, 0.5, i as f32 * 0.05);
            assert!(y <= 50.0 + 1e-3, "t={} y={y}", i as f32 * 0.05);
        }
        // 衝突時刻は 1 秒。反発係数 0.5 なら次の頂点は床から 12.5 上（= 落下高さの 1/4）
        let apex = (0..100).map(|i| bounce_y(0.0, 0.0, 100.0, 50.0, 0.5, 1.0 + i as f32 * 0.01)).fold(f32::MAX, f32::min);
        assert!((apex - 37.5).abs() < 0.2, "{apex}");
    }
}
