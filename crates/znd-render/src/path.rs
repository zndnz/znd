//! ベクターパスの幾何とラスタライズ。
//!
//! 塗りは「符号付き面積の累積」方式（フォントラスタライザーと同じ）で、辺ごとに各画素の
//! 被覆面積を解析的に積む。スーパーサンプリングなしで正確なアンチエイリアスが得られる。

use znd_core::shape::{BezPath, FillRule, LineCap, LineJoin};
use znd_core::Affine;

pub type Pt = [f64; 2];

#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub pts: Vec<Pt>,
    pub closed: bool,
}

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: Pt, b: Pt) -> Pt {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: Pt, k: f64) -> Pt {
    [a[0] * k, a[1] * k]
}
pub fn len(a: Pt) -> f64 {
    a[0].hypot(a[1])
}
fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
/// 進行方向 `d` の左法線（Y 下向き座標系で画面上は右手側）。長さ 1。
fn normal(d: Pt) -> Pt {
    let l = len(d);
    if l < 1e-12 {
        [0.0, 0.0]
    } else {
        [-d[1] / l, d[0] / l]
    }
}

pub fn cubic(c: &[Pt; 4], t: f64) -> Pt {
    let m = 1.0 - t;
    let (a, b, cc, d) = (m * m * m, 3.0 * m * m * t, 3.0 * m * t * t, t * t * t);
    [a * c[0][0] + b * c[1][0] + cc * c[2][0] + d * c[3][0], a * c[0][1] + b * c[1][1] + cc * c[2][1] + d * c[3][1]]
}

/// ベジェパスを折れ線にする。`tol` は許容誤差（パスの座標系での長さ）。
pub fn flatten(path: &BezPath, tol: f64) -> Polyline {
    let mut pts = Vec::new();
    let n = path.segment_count();
    if let Some(v) = path.vertices.first() {
        pts.push(v.p);
    }
    for i in 0..n {
        let c = path.segment(i);
        let straight = c[0] == c[1] && c[2] == c[3];
        let steps = if straight {
            1
        } else {
            // 2 階差分の大きさから必要分割数を見積もる
            let d1 = len(add(sub(c[0], mul(c[1], 2.0)), c[2]));
            let d2 = len(add(sub(c[1], mul(c[2], 2.0)), c[3]));
            ((0.75 * d1.max(d2) / tol.max(1e-6)).sqrt().ceil() as usize).clamp(1, 256)
        };
        for s in 1..=steps {
            pts.push(cubic(&c, s as f64 / steps as f64));
        }
    }
    if path.closed && pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    Polyline { pts, closed: path.closed }
}

impl Polyline {
    pub fn transformed(&self, m: &Affine) -> Polyline {
        Polyline {
            pts: self
                .pts
                .iter()
                .map(|p| {
                    let (x, y) = m.apply(p[0], p[1]);
                    [x, y]
                })
                .collect(),
            closed: self.closed,
        }
    }

    /// 辺の列（閉じたパスは最後→最初を含む）。
    pub fn edges(&self) -> impl Iterator<Item = (Pt, Pt)> + '_ {
        let n = self.pts.len();
        let count = if n < 2 {
            0
        } else if self.closed {
            n
        } else {
            n - 1
        };
        (0..count).map(move |i| (self.pts[i], self.pts[(i + 1) % n]))
    }

    pub fn length(&self) -> f64 {
        self.edges().map(|(a, b)| len(sub(b, a))).sum()
    }

    /// 弧長 `s0..s1` の部分（開いた折れ線）。
    pub fn slice(&self, s0: f64, s1: f64) -> Polyline {
        let mut out = Vec::new();
        let mut acc = 0.0;
        for (a, b) in self.edges() {
            let l = len(sub(b, a));
            let (e0, e1) = (acc, acc + l);
            if e1 > s0 && e0 < s1 && l > 0.0 {
                let t0 = ((s0 - e0) / l).clamp(0.0, 1.0);
                let t1 = ((s1 - e0) / l).clamp(0.0, 1.0);
                if out.is_empty() {
                    out.push(lerp(a, b, t0));
                }
                out.push(lerp(a, b, t1));
            }
            acc = e1;
        }
        Polyline { pts: out, closed: false }
    }

    /// 最大間隔 `step` になるよう辺を細分する。
    pub fn densify(&self, step: f64) -> Polyline {
        let mut pts = Vec::new();
        for (a, b) in self.edges() {
            let n = ((len(sub(b, a)) / step.max(1e-6)).ceil() as usize).clamp(1, 4096);
            for i in 0..n {
                pts.push(lerp(a, b, i as f64 / n as f64));
            }
        }
        if !self.closed {
            if let Some(last) = self.pts.last() {
                pts.push(*last);
            }
        }
        Polyline { pts, closed: self.closed }
    }

    /// 各頂点での単位法線（前後の辺の平均）。
    pub fn normals(&self) -> Vec<Pt> {
        let n = self.pts.len();
        (0..n)
            .map(|i| {
                let prev = if i > 0 {
                    Some(self.pts[i - 1])
                } else if self.closed {
                    Some(self.pts[n - 1])
                } else {
                    None
                };
                let next = if i + 1 < n {
                    Some(self.pts[i + 1])
                } else if self.closed {
                    Some(self.pts[0])
                } else {
                    None
                };
                let d = match (prev, next) {
                    (Some(a), Some(b)) => sub(b, a),
                    (Some(a), None) => sub(self.pts[i], a),
                    (None, Some(b)) => sub(b, self.pts[i]),
                    (None, None) => [1.0, 0.0],
                };
                normal(d)
            })
            .collect()
    }

    pub fn signed_area(&self) -> f64 {
        let n = self.pts.len();
        (0..n)
            .map(|i| {
                let (a, b) = (self.pts[i], self.pts[(i + 1) % n]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum::<f64>()
            * 0.5
    }

    /// パスを法線方向に `d` だけ太らせる（負で細らせる）。マイター結合。
    pub fn offset(&self, d: f64) -> Polyline {
        let n = self.pts.len();
        if n < 2 {
            return self.clone();
        }
        // 外向きが正になるよう、巻き方向で符号を合わせる
        let sign = if self.closed && self.signed_area() < 0.0 { 1.0 } else { -1.0 };
        let pts = (0..n)
            .map(|i| {
                let p = self.pts[i];
                let prev = if i > 0 { Some(self.pts[i - 1]) } else { self.closed.then(|| self.pts[n - 1]) };
                let next = if i + 1 < n { Some(self.pts[i + 1]) } else { self.closed.then(|| self.pts[0]) };
                let n0 = prev.map(|a| normal(sub(p, a)));
                let n1 = next.map(|b| normal(sub(b, p)));
                let m = match (n0, n1) {
                    (Some(a), Some(b)) => {
                        let s = add(a, b);
                        let l2 = s[0] * s[0] + s[1] * s[1];
                        // マイター長 = 1/cos(半角)。鋭角では 4 倍で打ち切る
                        if l2 < 1e-9 {
                            a
                        } else {
                            mul(s, (2.0 / l2).min(4.0))
                        }
                    }
                    (Some(a), None) | (None, Some(a)) => a,
                    (None, None) => [0.0, 0.0],
                };
                add(p, mul(m, d * sign))
            })
            .collect();
        Polyline { pts, closed: self.closed }
    }
}

/// 破線に分割する。`pattern` は線・間隔の繰り返し。
pub fn dash(line: &Polyline, pattern: &[f64], offset: f64) -> Vec<Polyline> {
    let period: f64 = pattern.iter().sum();
    if pattern.is_empty() || period <= 0.0 || pattern.iter().any(|v| *v < 0.0) {
        return vec![line.clone()];
    }
    // 奇数個のパターンは 2 周で 1 周期（SVG と同じ）
    let pat: Vec<f64> = if pattern.len() % 2 == 1 { pattern.iter().chain(pattern).copied().collect() } else { pattern.to_vec() };
    let period: f64 = pat.iter().sum();
    let total = line.length();
    let mut out = Vec::new();
    let mut pos = -offset.rem_euclid(period);
    let mut i = 0;
    while pos < total {
        let l = pat[i % pat.len()];
        if i % 2 == 0 && pos + l > 0.0 && l > 0.0 {
            let piece = line.slice(pos.max(0.0), (pos + l).min(total));
            if piece.pts.len() >= 2 {
                out.push(piece);
            }
        }
        pos += l;
        i += 1;
        if i > 100_000 {
            break;
        }
    }
    out
}

fn circle(c: Pt, r: f64, tol: f64) -> Polyline {
    let n = ((std::f64::consts::PI / (1.0 - (tol / r.max(tol)).min(0.5)).acos()).ceil() as usize).clamp(8, 128);
    Polyline {
        pts: (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            })
            .collect(),
        closed: true,
    }
}

fn positive(mut p: Polyline) -> Polyline {
    if p.signed_area() < 0.0 {
        p.pts.reverse();
    }
    p
}

/// 線の輪郭を、同じ向きの多角形の集まりとして返す（重なりは非ゼロ塗りで 1 つにまとまる）。
pub fn stroke_outline(line: &Polyline, width: f64, cap: LineCap, join: LineJoin, miter_limit: f64, tol: f64) -> Vec<Polyline> {
    let h = width * 0.5;
    let mut out = Vec::new();
    if h <= 0.0 || line.pts.is_empty() {
        return out;
    }
    // 長さ 0 の辺を除く
    let mut pts: Vec<Pt> = Vec::with_capacity(line.pts.len());
    for p in &line.pts {
        if pts.last().is_none_or(|q| len(sub(*p, *q)) > 1e-9) {
            pts.push(*p);
        }
    }
    if line.closed && pts.len() > 1 && len(sub(pts[0], pts[pts.len() - 1])) <= 1e-9 {
        pts.pop();
    }
    let n = pts.len();
    if n == 1 {
        match cap {
            LineCap::Round => out.push(circle(pts[0], h, tol)),
            LineCap::Square => out.push(Polyline {
                pts: vec![
                    [pts[0][0] - h, pts[0][1] - h],
                    [pts[0][0] + h, pts[0][1] - h],
                    [pts[0][0] + h, pts[0][1] + h],
                    [pts[0][0] - h, pts[0][1] + h],
                ],
                closed: true,
            }),
            LineCap::Butt => {}
        }
        return out;
    }
    let seg_count = if line.closed { n } else { n - 1 };
    for i in 0..seg_count {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let d = sub(b, a);
        let l = len(d);
        let u = mul(d, 1.0 / l);
        let nn = mul(normal(d), h);
        // 四角キャップは端の辺を h だけ延ばす
        let ext_a = if !line.closed && i == 0 && cap == LineCap::Square { h } else { 0.0 };
        let ext_b = if !line.closed && i == seg_count - 1 && cap == LineCap::Square { h } else { 0.0 };
        let a2 = sub(a, mul(u, ext_a));
        let b2 = add(b, mul(u, ext_b));
        out.push(positive(Polyline { pts: vec![add(a2, nn), add(b2, nn), sub(b2, nn), sub(a2, nn)], closed: true }));
    }
    // 結合
    let joins: Vec<usize> = if line.closed { (0..n).collect() } else { (1..n - 1).collect() };
    for i in joins {
        let p = pts[i];
        let prev = pts[(i + n - 1) % n];
        let next = pts[(i + 1) % n];
        let (d0, d1) = (sub(p, prev), sub(next, p));
        let cross = d0[0] * d1[1] - d0[1] * d1[0];
        if cross.abs() < 1e-12 {
            // 一直線、または折り返し。折り返しは丸結合のときだけ埋める
            if join == LineJoin::Round && d0[0] * d1[0] + d0[1] * d1[1] < 0.0 {
                out.push(circle(p, h, tol));
            }
            continue;
        }
        match join {
            LineJoin::Round => out.push(circle(p, h, tol)),
            LineJoin::Miter | LineJoin::Bevel => {
                // 外側（曲がる向きの反対側）の法線
                let s = if cross > 0.0 { -1.0 } else { 1.0 };
                let (n0, n1) = (mul(normal(d0), h * s), mul(normal(d1), h * s));
                let (o0, o1) = (add(p, n0), add(p, n1));
                let mut poly = vec![p, o0];
                if join == LineJoin::Miter {
                    let sum = add(n0, n1);
                    let l2 = sum[0] * sum[0] + sum[1] * sum[1];
                    // マイター先端 = p + sum * (2h² / |sum|²)。長さ/h がリミットを超えたらベベルにする
                    if l2 > 1e-12 {
                        let k = 2.0 * h * h / l2;
                        let tip = add(p, mul(sum, k));
                        if len(sub(tip, p)) / h <= miter_limit {
                            poly.push(tip);
                        }
                    }
                }
                poly.push(o1);
                out.push(positive(Polyline { pts: poly, closed: true }));
            }
        }
    }
    if !line.closed && cap == LineCap::Round {
        out.push(circle(pts[0], h, tol));
        out.push(circle(pts[n - 1], h, tol));
    }
    out
}

/// 被覆率マスク（キャンバス上の矩形領域）。
#[derive(Debug, Clone)]
pub struct Coverage {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Coverage {
    pub fn empty() -> Self {
        Coverage { x0: 0, y0: 0, w: 0, h: 0, data: Vec::new() }
    }

    /// キャンバス座標での値。領域外は 0。
    pub fn at(&self, x: usize, y: usize) -> f32 {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            0.0
        } else {
            self.data[(y - self.y0) * self.w + (x - self.x0)]
        }
    }

    /// キャンバス全体（`w`×`h`）の密な配列に展開する。
    pub fn to_dense(&self, w: usize, h: usize) -> Vec<f32> {
        let mut out = vec![0.0; w * h];
        for row in 0..self.h {
            let dst = (self.y0 + row) * w + self.x0;
            out[dst..dst + self.w].copy_from_slice(&self.data[row * self.w..(row + 1) * self.w]);
        }
        out
    }
}

struct Accum {
    w: usize,
    h: usize,
    /// 1 行あたり w+2 要素（右端からはみ出す寄与の受け皿）。
    a: Vec<f32>,
}

impl Accum {
    fn stride(&self) -> usize {
        self.w + 2
    }

    /// 領域内に収まっている線分（0 <= x <= w, 0 <= y <= h）の寄与を積む。
    fn line(&mut self, p0: [f32; 2], p1: [f32; 2]) {
        if p0[1] == p1[1] {
            return;
        }
        let (dir, p0, p1) = if p0[1] < p1[1] { (1.0f32, p0, p1) } else { (-1.0f32, p1, p0) };
        let dxdy = (p1[0] - p0[0]) / (p1[1] - p0[1]);
        let mut x = p0[0];
        let y_start = p0[1] as usize;
        let y_end = (p1[1].ceil() as usize).min(self.h);
        let stride = self.stride();
        for y in y_start..y_end {
            let row = y * stride;
            let dy = ((y + 1) as f32).min(p1[1]) - (y as f32).max(p0[1]);
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0floor = x0.floor();
            let x0i = x0floor as usize;
            let x1ceil = x1.ceil();
            let x1i = x1ceil as usize;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x + xnext) - x0floor;
                self.a[row + x0i] += d - d * xmf;
                self.a[row + x0i + 1] += d * xmf;
            } else {
                let s = (x1 - x0).recip();
                let x0f = x0 - x0floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                self.a[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.a[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.a[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.a[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.a[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                self.a[row + x1i] += d * am;
            }
            x = xnext;
        }
    }

    /// 任意の線分を領域にクリップして積む。左にはみ出た部分は x=0 上の縦線として扱う（巻き数を保つ）。
    fn add_edge(&mut self, a: Pt, b: Pt) {
        let (w, h) = (self.w as f64, self.h as f64);
        // y でクリップ
        let (mut a, mut b) = (a, b);
        if (a[1] <= 0.0 && b[1] <= 0.0) || (a[1] >= h && b[1] >= h) {
            return;
        }
        let clip_y = |p: Pt, q: Pt, y: f64| -> Pt { [p[0] + (q[0] - p[0]) * (y - p[1]) / (q[1] - p[1]), y] };
        if a[1] < 0.0 {
            a = clip_y(a, b, 0.0);
        } else if a[1] > h {
            a = clip_y(a, b, h);
        }
        if b[1] < 0.0 {
            b = clip_y(a, b, 0.0);
        } else if b[1] > h {
            b = clip_y(a, b, h);
        }
        // x の境界で分割し、各片を境界内に押し込む
        let mut cuts = vec![0.0f64, 1.0];
        for bound in [0.0, w] {
            if (a[0] < bound) != (b[0] < bound) && a[0] != b[0] {
                cuts.push((bound - a[0]) / (b[0] - a[0]));
            }
        }
        cuts.sort_by(|x, y| x.total_cmp(y));
        for pair in cuts.windows(2) {
            let (p, q) = (lerp(a, b, pair[0]), lerp(a, b, pair[1]));
            let mid = 0.5 * (p[0] + q[0]);
            let (px, qx) = if mid <= 0.0 {
                (0.0, 0.0)
            } else if mid >= w {
                (w, w)
            } else {
                (p[0].clamp(0.0, w), q[0].clamp(0.0, w))
            };
            self.line([px as f32, p[1] as f32], [qx as f32, q[1] as f32]);
        }
    }
}

/// 多角形群を塗った被覆率を求める。座標は出力画素。`out_w`×`out_h` にクリップする。
pub fn fill_coverage(polys: &[Polyline], rule: FillRule, out_w: usize, out_h: usize) -> Coverage {
    let mut min = [f64::MAX; 2];
    let mut max = [f64::MIN; 2];
    for p in polys.iter().flat_map(|p| &p.pts) {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    if min[0] > max[0] || !(min[0].is_finite() && max[0].is_finite() && min[1].is_finite() && max[1].is_finite()) {
        return Coverage::empty();
    }
    let x0 = (min[0].floor().max(0.0) as usize).min(out_w);
    let y0 = (min[1].floor().max(0.0) as usize).min(out_h);
    let x1 = (max[0].ceil().max(0.0) as usize + 1).min(out_w);
    let y1 = (max[1].ceil().max(0.0) as usize + 1).min(out_h);
    if x1 <= x0 || y1 <= y0 {
        return Coverage::empty();
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut acc = Accum { w, h, a: vec![0.0; (w + 2) * h + 2] };
    let off = [x0 as f64, y0 as f64];
    for poly in polys {
        let n = poly.pts.len();
        if n < 3 {
            continue;
        }
        // 塗りでは開いたパスも閉じて扱う
        for i in 0..n {
            acc.add_edge(sub(poly.pts[i], off), sub(poly.pts[(i + 1) % n], off));
        }
    }
    let stride = w + 2;
    let mut data = vec![0.0f32; w * h];
    for y in 0..h {
        let mut sum = 0.0f32;
        for x in 0..w {
            sum += acc.a[y * stride + x];
            let v = sum.abs();
            data[y * w + x] = match rule {
                FillRule::NonZero => v.min(1.0),
                // 偶奇: 巻き数の面積を三角波で折り返す
                FillRule::EvenOdd => 1.0 - ((v % 2.0) - 1.0).abs(),
            };
        }
    }
    Coverage { x0, y0, w, h, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(pts: &[Pt]) -> Polyline {
        Polyline { pts: pts.to_vec(), closed: true }
    }

    fn area(c: &Coverage) -> f64 {
        c.data.iter().map(|v| *v as f64).sum()
    }

    #[test]
    fn rectangle_coverage_is_exact() {
        let c = fill_coverage(&[poly(&[[2.25, 3.0], [6.25, 3.0], [6.25, 5.5], [2.25, 5.5]])], FillRule::NonZero, 16, 16);
        assert!((area(&c) - 10.0).abs() < 1e-4);
        assert!((c.at(2, 3) - 0.75).abs() < 1e-5);
        assert!((c.at(6, 4) - 0.25).abs() < 1e-5);
        assert!((c.at(3, 5) - 0.5).abs() < 1e-5);
        assert_eq!(c.at(3, 4), 1.0);
        assert_eq!(c.at(0, 0), 0.0);
        // 逆回りでも同じ
        let r = fill_coverage(&[poly(&[[2.25, 5.5], [6.25, 5.5], [6.25, 3.0], [2.25, 3.0]])], FillRule::NonZero, 16, 16);
        assert!((area(&r) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn triangle_and_circle_areas() {
        let c = fill_coverage(&[poly(&[[1.0, 1.0], [13.0, 2.0], [4.0, 14.0]])], FillRule::NonZero, 16, 16);
        assert!((area(&c) - 76.5).abs() < 1e-3, "{}", area(&c));
        let circ = flatten(&BezPath::ellipse([32.0, 32.0], [20.0, 20.0]), 0.01);
        let c = fill_coverage(&[circ], FillRule::NonZero, 64, 64);
        assert!((area(&c) - std::f64::consts::PI * 400.0).abs() / (std::f64::consts::PI * 400.0) < 2e-3);
    }

    #[test]
    fn clipping_keeps_winding() {
        // 画面の左・上・右・下にはみ出す矩形 → 画面内の部分だけが正しく塗られる
        let c = fill_coverage(&[poly(&[[-5.0, -5.0], [40.0, -5.0], [40.0, 4.5], [-5.0, 4.5]])], FillRule::NonZero, 8, 8);
        assert!((area(&c) - 36.0).abs() < 1e-4, "{}", area(&c));
        let c = fill_coverage(&[poly(&[[4.0, -10.0], [100.0, -10.0], [100.0, 100.0], [4.0, 100.0]])], FillRule::NonZero, 8, 8);
        assert!((area(&c) - 32.0).abs() < 1e-4, "{}", area(&c));
        assert_eq!(c.at(3, 3), 0.0);
        assert_eq!(c.at(7, 7), 1.0);
    }

    #[test]
    fn even_odd_makes_hole_nonzero_unions() {
        let outer = poly(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        let inner = poly(&[[2.0, 2.0], [8.0, 2.0], [8.0, 8.0], [2.0, 8.0]]);
        let eo = fill_coverage(&[outer.clone(), inner.clone()], FillRule::EvenOdd, 16, 16);
        assert!((area(&eo) - 64.0).abs() < 1e-3);
        let nz = fill_coverage(&[outer, inner], FillRule::NonZero, 16, 16);
        assert!((area(&nz) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn stroke_of_a_straight_line_has_expected_area() {
        let line = Polyline { pts: vec![[10.0, 20.0], [50.0, 20.0]], closed: false };
        let butt = fill_coverage(&stroke_outline(&line, 6.0, LineCap::Butt, LineJoin::Miter, 4.0, 0.05), FillRule::NonZero, 64, 64);
        assert!((area(&butt) - 240.0).abs() < 1e-3);
        let square = fill_coverage(&stroke_outline(&line, 6.0, LineCap::Square, LineJoin::Miter, 4.0, 0.05), FillRule::NonZero, 64, 64);
        assert!((area(&square) - 276.0).abs() < 1e-3);
        let round = fill_coverage(&stroke_outline(&line, 6.0, LineCap::Round, LineJoin::Miter, 4.0, 0.05), FillRule::NonZero, 64, 64);
        let expect = 240.0 + std::f64::consts::PI * 9.0;
        assert!((area(&round) - expect).abs() < 1.0, "{}", area(&round));
    }

    #[test]
    fn stroked_square_joins() {
        let sq = poly(&[[20.0, 20.0], [40.0, 20.0], [40.0, 40.0], [20.0, 40.0]]);
        // マイター: 外周 24x24、内周 16x16
        let miter = fill_coverage(&stroke_outline(&sq, 4.0, LineCap::Butt, LineJoin::Miter, 4.0, 0.05), FillRule::NonZero, 64, 64);
        assert!((area(&miter) - (576.0 - 256.0)).abs() < 1e-2, "{}", area(&miter));
        // ベベル: 四隅の三角 (2*2/2) が欠ける
        let bevel = fill_coverage(&stroke_outline(&sq, 4.0, LineCap::Butt, LineJoin::Bevel, 4.0, 0.05), FillRule::NonZero, 64, 64);
        assert!((area(&bevel) - (320.0 - 8.0)).abs() < 1e-2, "{}", area(&bevel));
        // 丸: 四隅が 1/4 円
        let round = fill_coverage(&stroke_outline(&sq, 4.0, LineCap::Butt, LineJoin::Round, 4.0, 0.02), FillRule::NonZero, 64, 64);
        let expect = 320.0 - 16.0 + std::f64::consts::PI * 4.0;
        assert!((area(&round) - expect).abs() < 0.3, "{}", area(&round));
    }

    #[test]
    fn dash_slice_offset() {
        let line = Polyline { pts: vec![[0.0, 0.0], [100.0, 0.0]], closed: false };
        let d = dash(&line, &[10.0, 5.0], 0.0);
        assert_eq!(d.len(), 7);
        assert_eq!(d[1].pts, vec![[15.0, 0.0], [25.0, 0.0]]);
        assert_eq!(d[6].pts, vec![[90.0, 0.0], [100.0, 0.0]]);
        let d = dash(&line, &[10.0, 5.0], 5.0);
        assert_eq!(d[0].pts, vec![[0.0, 0.0], [5.0, 0.0]]);
        let sq = poly(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        assert_eq!(sq.length(), 40.0);
        assert!((sq.offset(1.0).signed_area().abs() - 144.0).abs() < 1e-9);
        assert!((sq.offset(-1.0).signed_area().abs() - 64.0).abs() < 1e-9);
        assert_eq!(sq.slice(5.0, 15.0).pts, vec![[5.0, 0.0], [10.0, 0.0], [10.0, 5.0]]);
    }
}
