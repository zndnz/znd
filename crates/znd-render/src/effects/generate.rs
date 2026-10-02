//! 生成系: フラクタルノイズ、グラデーション、セル模様、チェッカー、オーディオ表示、線、落書き。

use super::{blend_with_original, fbm, generate, map_pixels, noise3, rand01, FxContext};
use crate::frame::FrameBuf;
use crate::path::{fill_coverage, flatten, stroke_outline, Polyline};
use crate::shape::{paint_coverage, Paint};
use znd_core::dsp::magnitude_spectrum;
use znd_core::effect::EvalParams;
use znd_core::shape::{FillRule, LineCap, LineJoin};

/// グレー値（リニア）の不透明画素。
fn gray(v: f32) -> [f32; 4] {
    [v, v, v, 1.0]
}

fn draw_lines(buf: &mut FrameBuf, lines: &[Polyline], width: f32, color: [f32; 4], cap: LineCap) {
    let polys: Vec<Polyline> = lines.iter().flat_map(|l| stroke_outline(l, width as f64, cap, LineJoin::Round, 4.0, 0.1)).collect();
    let cov = fill_coverage(&polys, FillRule::NonZero, buf.width, buf.height);
    paint_coverage(buf, &cov, &Paint::Solid(color), 1.0);
}

/// 最も近いセル中心までの距離（F1）と 2 番目（F2）、最も近いセルの乱数値。
fn worley(x: f32, y: f32, z: f32, seed: u32) -> (f32, f32, f32) {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2, mut id) = (f32::MAX, f32::MAX, 0.0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (xi + dx, yi + dy);
            let h = (cx as u32).wrapping_mul(73_856_093) ^ (cy as u32).wrapping_mul(19_349_663);
            // セル中心は「展開」に合わせて滑らかに漂う
            let px = cx as f32 + noise3(z, 0.0, 0.0, h ^ seed);
            let py = cy as f32 + noise3(z, 7.0, 0.0, h ^ seed ^ 0xabcd);
            let d = (x - px).hypot(y - py);
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = rand01(h, seed, 3);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

pub fn apply(kind: &str, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let s = ctx.scale;
    let (w, h) = (buf.width, buf.height);
    match kind {
        "fractal_noise" => {
            let scale = (p.f("scale") as f32 * s).max(0.5);
            let oct = p.f("complexity") as f32;
            let turbulent = p.i("fractal_type") == 1;
            let contrast = p.f("contrast") as f32 / 100.0;
            let brightness = p.f("brightness") as f32 / 100.0;
            let o = p.p("offset");
            let (ox, oy) = (o[0] as f32 * s, o[1] as f32 * s);
            let rev = p.f("evolution") as f32 / 360.0;
            let cycle = p.f("cycle") as f32;
            let looping = p.b("loop_evolution");
            let seed = ctx.seed;
            let value = |x: f32, y: f32| {
                let (nx, ny) = ((x - ox) / scale, (y - oy) / scale);
                if looping {
                    // 周回の端で同じ絵に戻るよう、1 周ずれた 2 つのノイズをクロスフェードする
                    let ph = (rev / cycle).rem_euclid(1.0);
                    let a = fbm(nx, ny, ph * cycle, oct, turbulent, seed);
                    let b = fbm(nx, ny, (ph - 1.0) * cycle, oct, turbulent, seed);
                    a + (b - a) * ph
                } else {
                    fbm(nx, ny, rev, oct, turbulent, seed)
                }
            };
            let noise = generate(w, h, |x, y| {
                let v = value(x as f32 + 0.5, y as f32 + 0.5);
                gray(((v - 0.5) * contrast + 0.5 + brightness).max(0.0))
            });
            let opacity = (p.f("opacity") as f32 / 100.0).clamp(0.0, 1.0);
            let mode = p.i("blend");
            map_pixels(&mut buf, |x, y, d| {
                let n = noise.pixel(x, y);
                let r = match mode {
                    1 => [d[0] + n[0], d[1] + n[1], d[2] + n[2], d[3].max(n[3])],
                    2 => [d[0] * n[0], d[1] * n[1], d[2] * n[2], d[3]],
                    3 => [d[0] + n[0] - d[0] * n[0], d[1] + n[1] - d[1] * n[1], d[2] + n[2] - d[2] * n[2], d[3].max(n[3])],
                    _ => n,
                };
                super::mix4(d, r, opacity)
            });
            buf
        }
        "gradient_ramp" => {
            let (a, b) = (p.p("start"), p.p("end"));
            let (ax, ay, bx, by) = (a[0] as f32 * s, a[1] as f32 * s, b[0] as f32 * s, b[1] as f32 * s);
            let (ca, cb) = (ctx.cx.premul(p.c("start_color")), ctx.cx.premul(p.c("end_color")));
            let radial = p.i("shape") == 1;
            let (dx, dy) = (bx - ax, by - ay);
            let l2 = (dx * dx + dy * dy).max(1e-9);
            let ramp = generate(w, h, |x, y| {
                let (px, py) = (x as f32 + 0.5 - ax, y as f32 + 0.5 - ay);
                let u = if radial { px.hypot(py) / l2.sqrt() } else { (px * dx + py * dy) / l2 };
                super::mix4(ca, cb, u.clamp(0.0, 1.0))
            });
            blend_with_original(&buf, ramp, p.f("opacity"))
        }
        "cell_pattern" => {
            let size = (p.f("size") as f32 * s).max(1.0);
            let z = p.f("evolution") as f32 / 360.0;
            let contrast = p.f("contrast") as f32 / 100.0;
            let mode = p.i("mode");
            let seed = ctx.seed;
            generate(w, h, |x, y| {
                let (f1, f2, id) = worley((x as f32 + 0.5) / size, (y as f32 + 0.5) / size, z, seed);
                let v = match mode {
                    1 => id,
                    2 => (f2 - f1).min(1.0),
                    _ => 1.0 - f1.min(1.0),
                };
                gray(((v - 0.5) * contrast + 0.5).clamp(0.0, 1.0))
            })
        }
        "checkerboard" => {
            let a = p.p("anchor");
            let (ax, ay) = (a[0] as f32 * s, a[1] as f32 * s);
            let (cw, ch) = ((p.f("width") as f32 * s).max(0.5), (p.f("height") as f32 * s).max(0.5));
            let col = ctx.cx.premul(p.c("color"));
            let opacity = (p.f("opacity") as f32 / 100.0).clamp(0.0, 1.0);
            map_pixels(&mut buf, |x, y, d| {
                let (cx, cy) = (((x as f32 + 0.5 - ax) / cw).floor() as i64, ((y as f32 + 0.5 - ay) / ch).floor() as i64);
                if (cx + cy).rem_euclid(2) == 0 {
                    let k = 1.0 - col[3] * opacity;
                    [col[0] * opacity + d[0] * k, col[1] * opacity + d[1] * k, col[2] * opacity + d[2] * k, col[3] * opacity + d[3] * k]
                } else {
                    d
                }
            });
            buf
        }
        "audio_spectrum" => {
            let Some(layer) = p.layer("audio_layer") else { return buf };
            const N: usize = 4096;
            let samples = (ctx.audio)(layer, N);
            if samples.len() < N {
                return buf;
            }
            let mag = magnitude_spectrum(&samples[..N]);
            let bands = p.f("bands").round().clamp(1.0, 1024.0) as usize;
            let (f0, f1) = (p.f("start_freq").max(1.0), p.f("end_freq").max(2.0));
            let (a, b) = (p.p("start"), p.p("end"));
            let (ax, ay, bx, by) = (a[0] * s as f64, a[1] * s as f64, b[0] * s as f64, b[1] * s as f64);
            let len = (bx - ax).hypot(by - ay).max(1e-9);
            // バーは基線に垂直に、画面上方向へ伸ばす
            let (nx, ny) = ((by - ay) / len, -(bx - ax) / len);
            let max_h = p.f("max_height") * s as f64;
            let hz_per_bin = 48_000.0 / N as f64;
            let lines: Vec<Polyline> = (0..bands)
                .map(|i| {
                    // 周波数は対数間隔（低域が潰れない）
                    let fa = f0 * (f1 / f0).powf(i as f64 / bands as f64);
                    let fb = f0 * (f1 / f0).powf((i + 1) as f64 / bands as f64);
                    let (ia, ib) = (
                        (fa / hz_per_bin).floor() as usize,
                        ((fb / hz_per_bin).ceil() as usize).max((fa / hz_per_bin).floor() as usize + 1),
                    );
                    let peak = mag[ia.min(mag.len() - 1)..ib.min(mag.len())].iter().fold(0.0f32, |m, v| m.max(*v)) as f64;
                    let u = (i as f64 + 0.5) / bands as f64;
                    let (x, y) = (ax + (bx - ax) * u, ay + (by - ay) * u);
                    let hgt = (peak * 2.0).min(1.0) * max_h;
                    Polyline { pts: vec![[x, y], [x + nx * hgt, y + ny * hgt]], closed: false }
                })
                .collect();
            draw_lines(&mut buf, &lines, p.f("thickness") as f32 * s, ctx.cx.premul(p.c("color")), LineCap::Butt);
            buf
        }
        "audio_waveform" => {
            let Some(layer) = p.layer("audio_layer") else { return buf };
            let n = p.f("samples").round().clamp(2.0, 4096.0) as usize;
            let samples = (ctx.audio)(layer, n);
            if samples.len() < n {
                return buf;
            }
            let (a, b) = (p.p("start"), p.p("end"));
            let (ax, ay, bx, by) = (a[0] * s as f64, a[1] * s as f64, b[0] * s as f64, b[1] * s as f64);
            let len = (bx - ax).hypot(by - ay).max(1e-9);
            let (nx, ny) = ((by - ay) / len, -(bx - ax) / len);
            let max_h = p.f("max_height") * s as f64 / 2.0;
            let pts = (0..n)
                .map(|i| {
                    let u = i as f64 / (n - 1) as f64;
                    let d = samples[i].clamp(-1.0, 1.0) as f64 * max_h;
                    [ax + (bx - ax) * u + nx * d, ay + (by - ay) * u + ny * d]
                })
                .collect();
            draw_lines(
                &mut buf,
                &[Polyline { pts, closed: false }],
                p.f("thickness") as f32 * s,
                ctx.cx.premul(p.c("color")),
                LineCap::Round,
            );
            buf
        }
        "stroke" => {
            let idx = (p.f("mask").round() as usize).saturating_sub(1);
            let Some(path) = ctx.masks.get(idx) else { return buf };
            let line = flatten(path, 0.1);
            let total = line.length();
            let (a, b) = (p.f("start").min(p.f("end")) / 100.0, p.f("start").max(p.f("end")) / 100.0);
            let piece = if a <= 0.0 && b >= 1.0 { line } else { line.slice(a * total, b * total) };
            if p.b("on_transparent") {
                buf.data.fill(0.0);
            }
            if piece.pts.len() >= 2 {
                draw_lines(&mut buf, &[piece], p.f("width") as f32 * s, ctx.cx.premul(p.c("color")), LineCap::Round);
            }
            buf
        }
        "scribble" => {
            let idx = (p.f("mask").round() as usize).saturating_sub(1);
            let Some(path) = ctx.masks.get(idx) else { return buf };
            let region = fill_coverage(&[flatten(path, 0.1)], FillRule::NonZero, w, h);
            if region.w == 0 {
                return buf;
            }
            let spacing = (p.f("spacing") * s as f64).max(1.0);
            let curv = p.f("curviness") * s as f64;
            let a = p.f("angle").to_radians();
            let (dx, dy) = (a.cos(), a.sin());
            let z = (ctx.time.seconds() * p.f("wiggles_per_second")) as f32;
            let seed = p.f("seed") as u32 ^ ctx.seed;
            // マスクの外接円を覆う平行線を引き、あとでマスクの内側だけ残す
            let (cx, cy) = (region.x0 as f64 + region.w as f64 / 2.0, region.y0 as f64 + region.h as f64 / 2.0);
            let reach = (region.w as f64).hypot(region.h as f64) / 2.0 + spacing;
            let count = (reach * 2.0 / spacing).ceil() as i64;
            let lines: Vec<Polyline> = (0..=count)
                .map(|k| {
                    let off = -reach + k as f64 * spacing;
                    let steps = (reach * 2.0 / 6.0).ceil() as usize;
                    let pts = (0..=steps)
                        .map(|i| {
                            let u = -reach + i as f64 * 6.0;
                            let wob = (noise3(i as f32 * 0.35, k as f32 * 3.1, z, seed) as f64 - 0.5) * 2.0 * curv;
                            [cx + dx * u - dy * (off + wob), cy + dy * u + dx * (off + wob)]
                        })
                        .collect();
                    Polyline { pts, closed: false }
                })
                .collect();
            let mut ink = FrameBuf::new(w, h);
            draw_lines(&mut ink, &lines, p.f("width") as f32 * s, ctx.cx.premul(p.c("color")), LineCap::Round);
            map_pixels(&mut buf, |x, y, d| {
                let i = ink.pixel(x, y).map(|v| v * region.at(x, y));
                let k = 1.0 - i[3];
                [i[0] + d[0] * k, i[1] + d[1] * k, i[2] + d[2] * k, i[3] + d[3] * k]
            });
            buf
        }
        _ => buf,
    }
}
