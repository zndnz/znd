//! シェイプレイヤーの評価と描画。
//!
//! グループ内の要素を上から順に評価する。図形はパスの集合に積まれ、パスオペレーターは
//! それまでのパスを書き換え、塗り・線はその時点のパスを描画命令にする。
//! 描画命令はリストの後ろほど下に描く（After Effects と同じ重なり順）。

use crate::color::ColorXf;
use crate::frame::FrameBuf;
use crate::path::{dash, fill_coverage, flatten, len, stroke_outline, Coverage, Polyline, Pt};
use znd_core::shape::{BezPath, FillRule, Gradient, MergeMode, PathVertex, ShapeItem, ShapeTransform};
use znd_core::{Affine, Time};

#[derive(Debug, Clone)]
pub enum Paint {
    Solid([f32; 4]),
    Gradient { radial: bool, start: Pt, end: Pt, stops: Vec<(f32, [f32; 4])> },
}

impl Paint {
    fn transformed(&self, m: &Affine) -> Paint {
        match self {
            Paint::Solid(c) => Paint::Solid(*c),
            Paint::Gradient { radial, start, end, stops } => {
                let (s, e) = (m.apply(start[0], start[1]), m.apply(end[0], end[1]));
                Paint::Gradient { radial: *radial, start: [s.0, s.1], end: [e.0, e.1], stops: stops.clone() }
            }
        }
    }

    fn at(&self, x: f64, y: f64) -> [f32; 4] {
        match self {
            Paint::Solid(c) => *c,
            Paint::Gradient { radial, start, end, stops } => {
                let d = [end[0] - start[0], end[1] - start[1]];
                let l2 = d[0] * d[0] + d[1] * d[1];
                let u = if l2 <= 0.0 {
                    0.0
                } else if *radial {
                    (len([x - start[0], y - start[1]]) / l2.sqrt()) as f32
                } else {
                    (((x - start[0]) * d[0] + (y - start[1]) * d[1]) / l2) as f32
                };
                gradient_color(stops, u)
            }
        }
    }
}

fn gradient_color(stops: &[(f32, [f32; 4])], u: f32) -> [f32; 4] {
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else { return [0.0; 4] };
    if u <= first.0 {
        return first.1;
    }
    if u >= last.0 {
        return last.1;
    }
    let i = stops.partition_point(|s| s.0 <= u).clamp(1, stops.len() - 1);
    let (a, b) = (&stops[i - 1], &stops[i]);
    let f = if b.0 > a.0 { (u - a.0) / (b.0 - a.0) } else { 0.0 };
    [a.1[0] + (b.1[0] - a.1[0]) * f, a.1[1] + (b.1[1] - a.1[1]) * f, a.1[2] + (b.1[2] - a.1[2]) * f, a.1[3] + (b.1[3] - a.1[3]) * f]
}

/// 被覆率に従ってペイントをキャンバスへ通常合成する。
pub fn paint_coverage(canvas: &mut FrameBuf, cov: &Coverage, paint: &Paint, opacity: f32) {
    if opacity <= 0.0 {
        return;
    }
    let w = canvas.width;
    for row in 0..cov.h {
        let y = cov.y0 + row;
        for col in 0..cov.w {
            let c = cov.data[row * cov.w + col] * opacity;
            if c <= 0.0 {
                continue;
            }
            let x = cov.x0 + col;
            let s = paint.at(x as f64 + 0.5, y as f64 + 0.5);
            let i = (y * w + x) * 4;
            let k = 1.0 - s[3] * c;
            for ch in 0..4 {
                canvas.data[i + ch] = s[ch] * c + canvas.data[i + ch] * k;
            }
        }
    }
}

struct Op {
    /// マージ指定があるときはパスごと、無いときは 1 要素にまとめた多角形群。
    parts: Vec<Vec<Polyline>>,
    merge: Option<MergeMode>,
    rule: FillRule,
    paint: Paint,
    opacity: f64,
}

impl Op {
    fn transformed(&self, m: &Affine, opacity: f64) -> Op {
        Op {
            parts: self.parts.iter().map(|polys| polys.iter().map(|p| p.transformed(m)).collect()).collect(),
            merge: self.merge,
            rule: self.rule,
            paint: self.paint.transformed(m),
            opacity: self.opacity * opacity,
        }
    }
}

fn shape_matrix(tr: &ShapeTransform, t: Time) -> Affine {
    let (a, p, s, r) = (tr.anchor.eval(t), tr.position.eval(t), tr.scale.eval(t), tr.rotation.eval(t));
    Affine::translate(p[0], p[1])
        .then_after(&Affine::rotate(r))
        .then_after(&Affine::scale(s[0] / 100.0, s[1] / 100.0))
        .then_after(&Affine::translate(-a[0], -a[1]))
}

pub fn transform_path(path: &BezPath, m: &Affine) -> BezPath {
    let lin = |v: [f64; 2]| [m.a * v[0] + m.c * v[1], m.b * v[0] + m.d * v[1]];
    BezPath {
        vertices: path
            .vertices
            .iter()
            .map(|v| {
                let (x, y) = m.apply(v.p[0], v.p[1]);
                PathVertex { p: [x, y], tan_in: lin(v.tan_in), tan_out: lin(v.tan_out) }
            })
            .collect(),
        closed: path.closed,
    }
}

fn from_polyline(p: &Polyline) -> BezPath {
    BezPath { vertices: p.pts.iter().map(|q| PathVertex::corner(q[0], q[1])).collect(), closed: p.closed }
}

/// 折れ線を通る滑らかなパス（Catmull-Rom 接線）。
fn smooth_from_polyline(p: &Polyline) -> BezPath {
    let n = p.pts.len();
    let vertices = (0..n)
        .map(|i| {
            let prev = if i > 0 { Some(p.pts[i - 1]) } else { p.closed.then(|| p.pts[n - 1]) };
            let next = if i + 1 < n { Some(p.pts[i + 1]) } else { p.closed.then(|| p.pts[0]) };
            let tan = match (prev, next) {
                (Some(a), Some(b)) => [(b[0] - a[0]) / 6.0, (b[1] - a[1]) / 6.0],
                _ => [0.0, 0.0],
            };
            PathVertex { p: p.pts[i], tan_in: [-tan[0], -tan[1]], tan_out: tan }
        })
        .collect();
    BezPath { vertices, closed: p.closed }
}

fn rect_path(size: [f64; 2], pos: [f64; 2], roundness: f64) -> BezPath {
    let (hw, hh) = (size[0] / 2.0, size[1] / 2.0);
    let r = roundness.clamp(0.0, hw.min(hh));
    if r <= 0.0 {
        return BezPath::rect(pos[0] - hw, pos[1] - hh, size[0], size[1]);
    }
    const K: f64 = 0.552_284_749_830_793_6;
    let k = r * K;
    let (l, t, rt, b) = (pos[0] - hw, pos[1] - hh, pos[0] + hw, pos[1] + hh);
    let v = |x: f64, y: f64, i: [f64; 2], o: [f64; 2]| PathVertex { p: [x, y], tan_in: i, tan_out: o };
    BezPath {
        vertices: vec![
            v(l + r, t, [-k, 0.0], [0.0, 0.0]),
            v(rt - r, t, [0.0, 0.0], [k, 0.0]),
            v(rt, t + r, [0.0, -k], [0.0, 0.0]),
            v(rt, b - r, [0.0, 0.0], [0.0, k]),
            v(rt - r, b, [k, 0.0], [0.0, 0.0]),
            v(l + r, b, [0.0, 0.0], [-k, 0.0]),
            v(l, b - r, [0.0, k], [0.0, 0.0]),
            v(l, t + r, [0.0, 0.0], [0.0, -k]),
        ],
        closed: true,
    }
}

fn polystar_path(star: bool, points: f64, pos: [f64; 2], rotation: f64, outer: f64, inner: f64) -> BezPath {
    let n = points.round().clamp(3.0, 256.0) as usize;
    let count = if star { n * 2 } else { n };
    let pts: Vec<[f64; 2]> = (0..count)
        .map(|i| {
            let r = if star && i % 2 == 1 { inner } else { outer };
            let a = (rotation - 90.0).to_radians() + i as f64 / count as f64 * std::f64::consts::TAU;
            [pos[0] + r * a.cos(), pos[1] + r * a.sin()]
        })
        .collect();
    BezPath::polygon(&pts)
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

fn rand_signed(a: u32, b: u32, c: u32) -> f64 {
    hash(a.wrapping_mul(0x9e37_79b1) ^ hash(b.wrapping_add(c.wrapping_mul(0x85eb_ca6b)))) as f64 / u32::MAX as f64 * 2.0 - 1.0
}

/// 時間方向に滑らかな乱数（-1..1）。
fn smooth_rand(seed: u32, lane: u32, x: f64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let s = f * f * (3.0 - 2.0 * f);
    let a = rand_signed(seed, lane, i as i64 as u32);
    let b = rand_signed(seed, lane, (i as i64 + 1) as u32);
    a + (b - a) * s
}

fn centroid(path: &BezPath) -> [f64; 2] {
    let n = path.vertices.len().max(1) as f64;
    let s = path.vertices.iter().fold([0.0, 0.0], |a, v| [a[0] + v.p[0], a[1] + v.p[1]]);
    [s[0] / n, s[1] / n]
}

fn trim(paths: &[BezPath], start: f64, end: f64, offset: f64, tol: f64) -> Vec<BezPath> {
    let off = offset / 360.0;
    let (mut s, mut e) = (start / 100.0, end / 100.0);
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    let (s, e) = (s.clamp(0.0, 1.0), e.clamp(0.0, 1.0));
    if e - s >= 1.0 {
        return paths.to_vec();
    }
    let mut out = Vec::new();
    if e - s <= 0.0 {
        return out;
    }
    // オフセットで範囲が 1 を超えたら先頭に回り込む
    let s2 = (s + off).rem_euclid(1.0);
    let e2 = s2 + (e - s);
    for path in paths {
        let line = flatten(path, tol);
        let total = line.length();
        let mut pieces = vec![line.slice(s2 * total, e2.min(1.0) * total)];
        if e2 > 1.0 {
            let wrapped = line.slice(0.0, (e2 - 1.0) * total);
            // 閉じたパスなら回り込み部分を 1 本につなぐ
            if path.closed {
                pieces[0].pts.extend(wrapped.pts.into_iter().skip(1));
            } else {
                pieces.push(wrapped);
            }
        }
        out.extend(pieces.into_iter().filter(|p| p.pts.len() >= 2).map(|p| from_polyline(&p)));
    }
    out
}

fn pucker_bloat(path: &BezPath, amount: f64) -> BezPath {
    let a = amount / 100.0;
    let c = centroid(path);
    let n = path.vertices.len();
    let toward = |p: [f64; 2], k: f64| [p[0] + (c[0] - p[0]) * k, p[1] + (c[1] - p[1]) * k];
    let vertices = (0..n)
        .map(|i| {
            let v = &path.vertices[i];
            // 頂点は中心へ（膨張）／外へ（パンク）、制御点はその逆へ動かす
            let p = toward(v.p, a);
            let prev = &path.vertices[(i + n - 1) % n];
            let next = &path.vertices[(i + 1) % n];
            // 接線が無い頂点は、辺の 1/3 点を制御点とみなす
            let abs_in = if v.tan_in == [0.0; 2] {
                [v.p[0] + (prev.p[0] - v.p[0]) / 3.0, v.p[1] + (prev.p[1] - v.p[1]) / 3.0]
            } else {
                [v.p[0] + v.tan_in[0], v.p[1] + v.tan_in[1]]
            };
            let abs_out = if v.tan_out == [0.0; 2] {
                [v.p[0] + (next.p[0] - v.p[0]) / 3.0, v.p[1] + (next.p[1] - v.p[1]) / 3.0]
            } else {
                [v.p[0] + v.tan_out[0], v.p[1] + v.tan_out[1]]
            };
            let (hi, ho) = (toward(abs_in, -a), toward(abs_out, -a));
            PathVertex { p, tan_in: [hi[0] - p[0], hi[1] - p[1]], tan_out: [ho[0] - p[0], ho[1] - p[1]] }
        })
        .collect();
    BezPath { vertices, closed: path.closed }
}

fn zigzag(path: &BezPath, size: f64, ridges: f64, smooth: bool, tol: f64) -> BezPath {
    let ridges = ridges.round().clamp(0.0, 512.0) as usize;
    let mut pts: Vec<Pt> = Vec::new();
    let segs = path.segment_count();
    let mut k = 0usize;
    for i in 0..segs {
        // 区間ごとに (ridges + 1) 等分し、分割点を交互に法線方向へずらす
        let seg = BezPath { vertices: vec![path.vertices[i], path.vertices[(i + 1) % path.vertices.len()]], closed: false };
        let line = flatten(&seg, tol);
        let total = line.length();
        let steps = ridges + 1;
        for s in 0..steps {
            let d = total * s as f64 / steps as f64;
            let piece = line.slice(d, (d + total * 1e-3).min(total));
            let Some(p) = piece.pts.first().copied() else { continue };
            let dir = piece.pts.last().map(|q| [q[0] - p[0], q[1] - p[1]]).unwrap_or([1.0, 0.0]);
            let l = len(dir).max(1e-12);
            let n = [-dir[1] / l, dir[0] / l];
            // 元の頂点（s == 0）は動かさない
            let amp = if s == 0 {
                0.0
            } else {
                k += 1;
                if k % 2 == 1 {
                    size
                } else {
                    -size
                }
            };
            pts.push([p[0] + n[0] * amp, p[1] + n[1] * amp]);
        }
    }
    if !path.closed {
        if let Some(v) = path.vertices.last() {
            pts.push(v.p);
        }
    }
    let line = Polyline { pts, closed: path.closed };
    if smooth {
        smooth_from_polyline(&line)
    } else {
        from_polyline(&line)
    }
}

fn displace_along_normal(path: &BezPath, step: f64, tol: f64, f: impl Fn(usize, f64) -> f64) -> BezPath {
    let line = flatten(path, tol).densify(step);
    let normals = line.normals();
    let mut s = 0.0;
    let mut pts = Vec::with_capacity(line.pts.len());
    for (i, p) in line.pts.iter().enumerate() {
        if i > 0 {
            s += len([p[0] - line.pts[i - 1][0], p[1] - line.pts[i - 1][1]]);
        }
        let d = f(i, s);
        pts.push([p[0] + normals[i][0] * d, p[1] + normals[i][1] * d]);
    }
    smooth_from_polyline(&Polyline { pts, closed: line.closed })
}

fn round_corners(path: &BezPath, radius: f64) -> BezPath {
    const K: f64 = 0.552_284_749_830_793_6;
    let n = path.vertices.len();
    if radius <= 0.0 || n < 3 {
        return path.clone();
    }
    let mut out = Vec::new();
    for i in 0..n {
        let v = &path.vertices[i];
        let ends = !path.closed && (i == 0 || i == n - 1);
        if ends || v.tan_in != [0.0; 2] || v.tan_out != [0.0; 2] {
            out.push(*v);
            continue;
        }
        let prev = path.vertices[(i + n - 1) % n].p;
        let next = path.vertices[(i + 1) % n].p;
        let (d0, d1) = ([prev[0] - v.p[0], prev[1] - v.p[1]], [next[0] - v.p[0], next[1] - v.p[1]]);
        let (l0, l1) = (len(d0), len(d1));
        if l0 < 1e-9 || l1 < 1e-9 {
            out.push(*v);
            continue;
        }
        // 隣の角と食い合わないよう、辺の半分までに制限する
        let r = radius.min(l0 / 2.0).min(l1 / 2.0);
        let (u0, u1) = ([d0[0] / l0, d0[1] / l0], [d1[0] / l1, d1[1] / l1]);
        let a = [v.p[0] + u0[0] * r, v.p[1] + u0[1] * r];
        let b = [v.p[0] + u1[0] * r, v.p[1] + u1[1] * r];
        out.push(PathVertex { p: a, tan_in: [0.0, 0.0], tan_out: [-u0[0] * r * K, -u0[1] * r * K] });
        out.push(PathVertex { p: b, tan_in: [-u1[0] * r * K, -u1[1] * r * K], tan_out: [0.0, 0.0] });
    }
    BezPath { vertices: out, closed: path.closed }
}

fn twist(paths: &[BezPath], angle: f64, center: [f64; 2], tol: f64) -> Vec<BezPath> {
    let lines: Vec<Polyline> = paths.iter().map(|p| flatten(p, tol).densify(4.0)).collect();
    let max_r = lines.iter().flat_map(|l| &l.pts).map(|p| len([p[0] - center[0], p[1] - center[1]])).fold(0.0, f64::max);
    if max_r <= 0.0 {
        return paths.to_vec();
    }
    lines
        .iter()
        .map(|l| {
            let pts = l
                .pts
                .iter()
                .map(|p| {
                    let d = [p[0] - center[0], p[1] - center[1]];
                    // 中心ほど大きく回す
                    let a = (angle * (1.0 - len(d) / max_r)).to_radians();
                    let (s, c) = a.sin_cos();
                    [center[0] + d[0] * c - d[1] * s, center[1] + d[0] * s + d[1] * c]
                })
                .collect();
            from_polyline(&Polyline { pts, closed: l.closed })
        })
        .collect()
}

fn make_paint(color: [f64; 4], gradient: &Option<Gradient>, t: Time, cx: &ColorXf) -> Paint {
    match gradient {
        None => Paint::Solid(cx.premul(color)),
        Some(g) => Paint::Gradient {
            radial: g.radial,
            start: g.start.eval(t),
            end: g.end.eval(t),
            stops: g.stops.iter().map(|(u, c)| (*u as f32, cx.premul(*c))).collect(),
        },
    }
}

/// グループを評価して (グループ座標のパス, 描画命令) を返す。`accum` はグループ座標 → 出力画素（許容誤差の見積もり用）。
fn eval_group(items: &[ShapeItem], t: Time, accum: &Affine, cx: &ColorXf) -> (Vec<BezPath>, Vec<Op>) {
    let scale = (accum.a * accum.d - accum.b * accum.c).abs().sqrt().max(1e-6);
    let tol = 0.03 / scale;
    let mut paths: Vec<BezPath> = Vec::new();
    let mut ops: Vec<Op> = Vec::new();
    let mut merge: Option<MergeMode> = None;

    for item in items {
        match item {
            ShapeItem::Group { items, transform, .. } => {
                let gm = shape_matrix(transform, t);
                let (gp, gops) = eval_group(items, t, &accum.then_after(&gm), cx);
                let opacity = (transform.opacity.eval(t) / 100.0).clamp(0.0, 1.0);
                paths.extend(gp.iter().map(|p| transform_path(p, &gm)));
                ops.extend(gops.iter().map(|o| o.transformed(&gm, opacity)));
            }
            ShapeItem::Rect { size, position, roundness } => paths.push(rect_path(size.eval(t), position.eval(t), roundness.eval(t))),
            ShapeItem::Ellipse { size, position } => {
                let s = size.eval(t);
                paths.push(BezPath::ellipse(position.eval(t), [s[0] / 2.0, s[1] / 2.0]));
            }
            ShapeItem::Polystar { star, points, position, rotation, outer_radius, inner_radius } => paths.push(polystar_path(
                *star,
                points.eval(t),
                position.eval(t),
                rotation.eval(t),
                outer_radius.eval(t),
                inner_radius.eval(t),
            )),
            ShapeItem::Path { path } => paths.push(path.eval(t)),
            ShapeItem::Fill { color, opacity, rule, gradient } => {
                let polys: Vec<Polyline> = paths.iter().map(|p| flatten(p, tol)).collect();
                let parts = if merge.is_some() { polys.into_iter().map(|p| vec![p]).collect() } else { vec![polys] };
                ops.push(Op {
                    parts,
                    merge,
                    rule: *rule,
                    paint: make_paint(color.eval(t), gradient, t, cx),
                    opacity: opacity.eval(t) / 100.0,
                });
            }
            ShapeItem::Stroke { color, opacity, width, cap, join, miter_limit, dashes, dash_offset, gradient } => {
                let w = width.eval(t);
                let mut polys = Vec::new();
                for p in &paths {
                    let line = flatten(p, tol);
                    let pieces = if dashes.is_empty() { vec![line] } else { dash(&line, dashes, dash_offset.eval(t)) };
                    for piece in pieces {
                        polys.extend(stroke_outline(&piece, w, *cap, *join, *miter_limit, tol));
                    }
                }
                ops.push(Op {
                    parts: vec![polys],
                    merge: None,
                    rule: FillRule::NonZero,
                    paint: make_paint(color.eval(t), gradient, t, cx),
                    opacity: opacity.eval(t) / 100.0,
                });
            }
            ShapeItem::Trim { start, end, offset } => paths = trim(&paths, start.eval(t), end.eval(t), offset.eval(t), tol),
            ShapeItem::Repeater { copies, offset, transform, end_opacity } => {
                let n = copies.eval(t).round().clamp(0.0, 1000.0) as usize;
                let off = offset.eval(t);
                let (a, p, s, r) =
                    (transform.anchor.eval(t), transform.position.eval(t), transform.scale.eval(t), transform.rotation.eval(t));
                let (o0, o1) = (transform.opacity.eval(t) / 100.0, end_opacity.eval(t) / 100.0);
                let mut new_paths = Vec::new();
                let mut new_ops = Vec::new();
                // 後のコピーほど下になるよう、命令は 0 番のコピーから並べる
                for i in 0..n {
                    let k = i as f64 + off;
                    let m = Affine::translate(p[0] * k + a[0], p[1] * k + a[1])
                        .then_after(&Affine::rotate(r * k))
                        .then_after(&Affine::scale((s[0] / 100.0).powf(k), (s[1] / 100.0).powf(k)))
                        .then_after(&Affine::translate(-a[0], -a[1]));
                    let f = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
                    new_paths.extend(paths.iter().map(|q| transform_path(q, &m)));
                    new_ops.extend(ops.iter().map(|o| o.transformed(&m, o0 + (o1 - o0) * f)));
                }
                paths = new_paths;
                ops = new_ops;
            }
            ShapeItem::OffsetPath { amount, .. } => {
                let d = amount.eval(t);
                paths = paths.iter().map(|p| from_polyline(&flatten(p, tol).offset(d))).collect();
            }
            ShapeItem::PuckerBloat { amount } => {
                let a = amount.eval(t);
                paths = paths.iter().map(|p| pucker_bloat(p, a)).collect();
            }
            ShapeItem::ZigZag { size, ridges, smooth } => {
                let (s, r) = (size.eval(t), ridges.eval(t));
                paths = paths.iter().map(|p| zigzag(p, s, r, *smooth, tol)).collect();
            }
            ShapeItem::Wave { amplitude, wavelength, phase } => {
                let (amp, wl, ph) = (amplitude.eval(t), wavelength.eval(t).max(1e-3), phase.eval(t).to_radians());
                paths = paths
                    .iter()
                    .map(|p| displace_along_normal(p, wl / 12.0, tol, |_, s| amp * (std::f64::consts::TAU * s / wl + ph).sin()))
                    .collect();
            }
            ShapeItem::WigglePath { size, detail, speed, seed } => {
                let (sz, det, sp) = (size.eval(t), detail.eval(t).max(1.0), speed.eval(t));
                let x = t.seconds() * sp;
                paths = paths
                    .iter()
                    .enumerate()
                    .map(|(pi, p)| displace_along_normal(p, det, tol, |i, _| sz * smooth_rand(*seed, (pi as u32) << 16 | i as u32, x)))
                    .collect();
            }
            ShapeItem::RoundCorners { radius } => {
                let r = radius.eval(t);
                paths = paths.iter().map(|p| round_corners(p, r)).collect();
            }
            ShapeItem::Merge { mode } => merge = Some(*mode),
            ShapeItem::Twist { angle, center } => paths = twist(&paths, angle.eval(t), center.eval(t), tol),
        }
    }
    (paths, ops)
}

fn op_coverage(op: &Op, w: usize, h: usize) -> Coverage {
    let Some(mode) = op.merge.filter(|_| op.parts.len() > 1) else {
        let all: Vec<Polyline> = op.parts.iter().flatten().cloned().collect();
        return fill_coverage(&all, op.rule, w, h);
    };
    // パスごとの被覆率を画素単位で合成する
    let mut acc = fill_coverage(&op.parts[0], op.rule, w, h).to_dense(w, h);
    for part in &op.parts[1..] {
        let c = fill_coverage(part, op.rule, w, h);
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (acc[y * w + x], c.at(x, y));
                acc[y * w + x] = match mode {
                    MergeMode::Add => a + b - a * b,
                    MergeMode::Subtract => a * (1.0 - b),
                    MergeMode::Intersect => a * b,
                    MergeMode::Exclude => a + b - 2.0 * a * b,
                };
            }
        }
    }
    Coverage { x0: 0, y0: 0, w, h, data: acc }
}

/// シェイプの内容をキャンバスに描く。`m` はレイヤー座標 → 出力画素。
pub fn render_shape(items: &[ShapeItem], t: Time, m: &Affine, canvas: &mut FrameBuf, cx: &ColorXf) {
    let (_, ops) = eval_group(items, t, m, cx);
    let (w, h) = (canvas.width, canvas.height);
    for op in ops.iter().rev() {
        let op = op.transformed(m, 1.0);
        let cov = op_coverage(&op, w, h);
        paint_coverage(canvas, &cov, &op.paint, op.opacity.clamp(0.0, 1.0) as f32);
    }
}

/// シェイプ内容の全パス（レイヤー座標）。パス上テキストやアウトライン確認に使う。
pub fn shape_paths(items: &[ShapeItem], t: Time) -> Vec<BezPath> {
    eval_group(items, t, &Affine::IDENTITY, &ColorXf::SRGB).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::Property;

    fn white() -> ShapeItem {
        ShapeItem::fill([1.0, 1.0, 1.0, 1.0])
    }

    fn alpha_area(items: &[ShapeItem], size: usize) -> f64 {
        let mut c = FrameBuf::new(size, size);
        let m = Affine::translate(size as f64 / 2.0, size as f64 / 2.0);
        render_shape(items, Time::ZERO, &m, &mut c, &ColorXf::SRGB);
        c.data.chunks_exact(4).map(|p| p[3] as f64).sum()
    }

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= b.abs() * rel
    }

    #[test]
    fn primitives_have_expected_areas() {
        assert!(close(alpha_area(&[ShapeItem::rect(40.0, 20.0), white()], 100), 800.0, 1e-4));
        assert!(close(alpha_area(&[ShapeItem::ellipse(60.0, 40.0), white()], 100), std::f64::consts::PI * 30.0 * 20.0, 3e-3));
        // 正六角形: (3√3/2)·r²
        assert!(close(alpha_area(&[ShapeItem::polygon(6.0, 30.0), white()], 100), 1.5 * 3f64.sqrt() * 900.0, 1e-3));
        // 角丸矩形: 面積 = w·h − (4 − π)r²
        let rr = ShapeItem::Rect {
            size: Property::constant([60.0, 40.0]),
            position: Property::constant([0.0, 0.0]),
            roundness: Property::constant(10.0),
        };
        assert!(close(alpha_area(&[rr, white()], 100), 2400.0 - (4.0 - std::f64::consts::PI) * 100.0, 3e-3));
    }

    #[test]
    fn stroke_draws_above_fill_and_group_transform_applies() {
        let items = vec![ShapeItem::Group {
            name: "g".into(),
            items: vec![ShapeItem::rect(20.0, 20.0), ShapeItem::stroke([1.0, 0.0, 0.0, 1.0], 4.0), ShapeItem::fill([0.0, 0.0, 1.0, 1.0])],
            transform: ShapeTransform { position: Property::constant([10.0, 0.0]), ..Default::default() },
        }];
        let mut c = FrameBuf::new(64, 64);
        render_shape(&items, Time::ZERO, &Affine::translate(32.0, 32.0), &mut c, &ColorXf::SRGB);
        // グループが +10 移動 → 矩形は x 32..52。中心は青、辺の上は赤（線が上）
        assert_eq!(c.pixel(42, 32), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(c.pixel(32, 32), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(c.pixel(25, 32), [0.0; 4]);
    }

    #[test]
    fn trim_path_length_is_proportional() {
        let line =
            ShapeItem::path(BezPath { vertices: vec![PathVertex::corner(-40.0, 0.0), PathVertex::corner(40.0, 0.0)], closed: false });
        let trimmed = |s: f64, e: f64| {
            vec![
                line.clone(),
                ShapeItem::Trim { start: Property::constant(s), end: Property::constant(e), offset: Property::constant(0.0) },
                ShapeItem::stroke([1.0; 4], 2.0),
            ]
        };
        assert!(close(alpha_area(&trimmed(0.0, 100.0), 100), 160.0, 1e-4));
        assert!(close(alpha_area(&trimmed(25.0, 75.0), 100), 80.0, 1e-4));
        assert_eq!(alpha_area(&trimmed(40.0, 40.0), 100), 0.0);
        // 閉じたパスでオフセットにより回り込んでも長さは同じ
        let ring = vec![
            ShapeItem::ellipse(60.0, 60.0),
            ShapeItem::Trim { start: Property::constant(0.0), end: Property::constant(50.0), offset: Property::constant(270.0) },
            ShapeItem::stroke([1.0; 4], 2.0),
        ];
        assert!(close(alpha_area(&ring, 100), std::f64::consts::PI * 30.0 * 2.0, 0.01));
    }

    #[test]
    fn repeater_copies_shapes() {
        let items = vec![
            ShapeItem::rect(10.0, 10.0),
            white(),
            ShapeItem::Repeater {
                copies: Property::constant(3.0),
                offset: Property::constant(0.0),
                transform: ShapeTransform { position: Property::constant([20.0, 0.0]), ..Default::default() },
                end_opacity: Property::constant(100.0),
            },
        ];
        assert!(close(alpha_area(&items, 120), 300.0, 1e-4));
    }

    #[test]
    fn merge_modes() {
        let two = |mode: MergeMode| {
            vec![
                ShapeItem::rect(40.0, 40.0),
                ShapeItem::Rect {
                    size: Property::constant([40.0, 40.0]),
                    position: Property::constant([20.0, 0.0]),
                    roundness: Property::constant(0.0),
                },
                ShapeItem::Merge { mode },
                white(),
            ]
        };
        assert!(close(alpha_area(&two(MergeMode::Add), 120), 2400.0, 1e-4));
        assert!(close(alpha_area(&two(MergeMode::Subtract), 120), 800.0, 1e-4));
        assert!(close(alpha_area(&two(MergeMode::Intersect), 120), 800.0, 1e-4));
        assert!(close(alpha_area(&two(MergeMode::Exclude), 120), 1600.0, 1e-4));
    }

    #[test]
    fn operators_change_geometry_plausibly() {
        let base = alpha_area(&[ShapeItem::rect(40.0, 40.0), white()], 120);
        let with = |op: ShapeItem| alpha_area(&[ShapeItem::rect(40.0, 40.0), op, white()], 120);
        assert!(close(with(ShapeItem::OffsetPath { amount: Property::constant(5.0), join: Default::default() }), 2500.0, 1e-4));
        // 角丸: 面積 = 1600 − (4 − π)·r²
        assert!(close(
            with(ShapeItem::RoundCorners { radius: Property::constant(10.0) }),
            1600.0 - (4.0 - std::f64::consts::PI) * 100.0,
            3e-3
        ));
        // 膨張は辺が外へ膨らみ頂点が内へ入る。パンクは逆で面積が減る
        assert!(with(ShapeItem::PuckerBloat { amount: Property::constant(-50.0) }) < base);
        // ジグザグ・波形・ねじれは面積をほぼ保つ
        assert!(close(
            with(ShapeItem::ZigZag { size: Property::constant(3.0), ridges: Property::constant(4.0), smooth: false }),
            base,
            0.05
        ));
        assert!(close(
            with(ShapeItem::Wave {
                amplitude: Property::constant(2.0),
                wavelength: Property::constant(20.0),
                phase: Property::constant(0.0)
            }),
            base,
            0.05
        ));
        assert!(close(with(ShapeItem::Twist { angle: Property::constant(90.0), center: Property::constant([0.0, 0.0]) }), base, 0.2));
        // ウィグルは時刻で形が変わり、同じ時刻なら同じ形
        let wig = vec![
            ShapeItem::rect(40.0, 40.0),
            ShapeItem::WigglePath {
                size: Property::constant(5.0),
                detail: Property::constant(8.0),
                speed: Property::constant(2.0),
                seed: 1,
            },
            white(),
        ];
        let render = |t: f64| {
            let mut c = FrameBuf::new(120, 120);
            render_shape(&wig, Time::from_seconds(t), &Affine::translate(60.0, 60.0), &mut c, &ColorXf::SRGB);
            c
        };
        assert_eq!(render(0.3), render(0.3));
        assert_ne!(render(0.3), render(0.8));
    }

    #[test]
    fn gradient_fill_interpolates_in_linear_light() {
        let g = Gradient {
            radial: false,
            start: Property::constant([-50.0, 0.0]),
            end: Property::constant([50.0, 0.0]),
            stops: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])],
        };
        let items = vec![
            ShapeItem::rect(100.0, 20.0),
            ShapeItem::Fill {
                color: Property::constant([1.0; 4]),
                opacity: Property::constant(100.0),
                rule: FillRule::NonZero,
                gradient: Some(g),
            },
        ];
        let mut c = FrameBuf::new(100, 40);
        render_shape(&items, Time::ZERO, &Affine::translate(50.0, 20.0), &mut c, &ColorXf::SRGB);
        assert!((c.pixel(49, 20)[0] - 0.495).abs() < 1e-3);
        assert!((c.pixel(10, 20)[0] - 0.105).abs() < 1e-3);
    }
}
