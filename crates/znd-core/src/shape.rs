//! ベジェパス、シェイプレイヤーの内容、マスク。

use crate::property::{Animatable, Property};
use serde::{Deserialize, Serialize};

/// パスの頂点。接線は頂点からの相対ベクトル。JSON では `[x, y, in_x, in_y, out_x, out_y]`。
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f64; 6]", into = "[f64; 6]")]
pub struct PathVertex {
    pub p: [f64; 2],
    pub tan_in: [f64; 2],
    pub tan_out: [f64; 2],
}

impl From<[f64; 6]> for PathVertex {
    fn from(a: [f64; 6]) -> Self {
        PathVertex { p: [a[0], a[1]], tan_in: [a[2], a[3]], tan_out: [a[4], a[5]] }
    }
}

impl From<PathVertex> for [f64; 6] {
    fn from(v: PathVertex) -> Self {
        [v.p[0], v.p[1], v.tan_in[0], v.tan_in[1], v.tan_out[0], v.tan_out[1]]
    }
}

impl PathVertex {
    pub fn corner(x: f64, y: f64) -> Self {
        PathVertex { p: [x, y], tan_in: [0.0; 2], tan_out: [0.0; 2] }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BezPath {
    pub vertices: Vec<PathVertex>,
    #[serde(default)]
    pub closed: bool,
}

impl BezPath {
    pub fn polygon(points: &[[f64; 2]]) -> Self {
        BezPath { vertices: points.iter().map(|p| PathVertex::corner(p[0], p[1])).collect(), closed: true }
    }

    pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self::polygon(&[[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
    }

    /// 中心 `c`・半径 `r` の楕円（4 頂点のベジェ近似）。
    pub fn ellipse(c: [f64; 2], r: [f64; 2]) -> Self {
        const K: f64 = 0.552_284_749_830_793_6;
        let v = |dx: f64, dy: f64, tx: f64, ty: f64| PathVertex {
            p: [c[0] + dx * r[0], c[1] + dy * r[1]],
            tan_in: [-tx * r[0] * K, -ty * r[1] * K],
            tan_out: [tx * r[0] * K, ty * r[1] * K],
        };
        BezPath {
            vertices: vec![v(0.0, -1.0, 1.0, 0.0), v(1.0, 0.0, 0.0, 1.0), v(0.0, 1.0, -1.0, 0.0), v(-1.0, 0.0, 0.0, -1.0)],
            closed: true,
        }
    }

    /// 区間数（閉じたパスは最後→最初も含む）。
    pub fn segment_count(&self) -> usize {
        match (self.vertices.len(), self.closed) {
            (0, _) => 0,
            (n, true) => n,
            (n, false) => n - 1,
        }
    }

    /// 区間 `i` の 3 次ベジェ制御点。
    pub fn segment(&self, i: usize) -> [[f64; 2]; 4] {
        let a = &self.vertices[i];
        let b = &self.vertices[(i + 1) % self.vertices.len()];
        [a.p, [a.p[0] + a.tan_out[0], a.p[1] + a.tan_out[1]], [b.p[0] + b.tan_in[0], b.p[1] + b.tan_in[1]], b.p]
    }
}

impl Animatable for BezPath {
    fn dims(&self) -> usize {
        self.vertices.len() * 6
    }
    fn component(&self, i: usize) -> f64 {
        let a: [f64; 6] = self.vertices[i / 6].into();
        a[i % 6]
    }
    fn with_components(&self, c: &[f64]) -> Self {
        BezPath {
            vertices: c.chunks_exact(6).map(|v| PathVertex::from([v[0], v[1], v[2], v[3], v[4], v[5]])).collect(),
            closed: self.closed,
        }
    }
    fn compatible(&self, other: &Self) -> bool {
        self.vertices.len() == other.vertices.len() && self.closed == other.closed
    }
}

// ---------- シェイプレイヤー ----------

fn p1(v: f64) -> Property<f64> {
    Property::constant(v)
}
fn p2(x: f64, y: f64) -> Property<[f64; 2]> {
    Property::constant([x, y])
}
fn d0() -> Property<f64> {
    p1(0.0)
}
fn d100() -> Property<f64> {
    p1(100.0)
}
fn d00() -> Property<[f64; 2]> {
    p2(0.0, 0.0)
}
fn d100x() -> Property<[f64; 2]> {
    p2(100.0, 100.0)
}

/// グループ／リピーターのトランスフォーム。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShapeTransform {
    pub anchor: Property<[f64; 2]>,
    pub position: Property<[f64; 2]>,
    pub scale: Property<[f64; 2]>,
    pub rotation: Property<f64>,
    pub opacity: Property<f64>,
}

impl Default for ShapeTransform {
    fn default() -> Self {
        ShapeTransform { anchor: d00(), position: d00(), scale: d100x(), rotation: d0(), opacity: d100() }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub radial: bool,
    pub start: Property<[f64; 2]>,
    pub end: Property<[f64; 2]>,
    /// (位置 0..1, sRGB 色 RGBA)。位置の昇順。
    pub stops: Vec<(f64, [f64; 4])>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// 結合（和）。
    #[default]
    Add,
    /// 最初のパスから残りを引く。
    Subtract,
    Intersect,
    /// 重なりを除外（XOR）。
    Exclude,
}

/// シェイプレイヤーの要素。グループ内では上から順に評価し、塗り・線はそれより上にあるパスを描く。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeItem {
    Group {
        #[serde(default)]
        name: String,
        items: Vec<ShapeItem>,
        #[serde(default)]
        transform: ShapeTransform,
    },
    Rect {
        size: Property<[f64; 2]>,
        #[serde(default = "d00")]
        position: Property<[f64; 2]>,
        #[serde(default = "d0")]
        roundness: Property<f64>,
    },
    Ellipse {
        size: Property<[f64; 2]>,
        #[serde(default = "d00")]
        position: Property<[f64; 2]>,
    },
    /// 多角形（`star == false`）またはスター。
    Polystar {
        star: bool,
        points: Property<f64>,
        #[serde(default = "d00")]
        position: Property<[f64; 2]>,
        #[serde(default = "d0")]
        rotation: Property<f64>,
        outer_radius: Property<f64>,
        #[serde(default = "d0")]
        inner_radius: Property<f64>,
    },
    Path {
        path: Property<BezPath>,
    },
    Fill {
        /// sRGB RGBA。
        color: Property<[f64; 4]>,
        #[serde(default = "d100")]
        opacity: Property<f64>,
        #[serde(default)]
        rule: FillRule,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gradient: Option<Gradient>,
    },
    Stroke {
        color: Property<[f64; 4]>,
        #[serde(default = "d100")]
        opacity: Property<f64>,
        width: Property<f64>,
        #[serde(default)]
        cap: LineCap,
        #[serde(default)]
        join: LineJoin,
        #[serde(default = "four")]
        miter_limit: f64,
        /// 破線パターン（線・間隔の繰り返し）。空なら実線。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dashes: Vec<f64>,
        #[serde(default = "d0")]
        dash_offset: Property<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gradient: Option<Gradient>,
    },
    /// トリムパス。値はパーセント、`offset` は度。
    Trim {
        start: Property<f64>,
        end: Property<f64>,
        #[serde(default = "d0")]
        offset: Property<f64>,
    },
    Repeater {
        copies: Property<f64>,
        #[serde(default = "d0")]
        offset: Property<f64>,
        #[serde(default)]
        transform: ShapeTransform,
        #[serde(default = "d100")]
        end_opacity: Property<f64>,
    },
    OffsetPath {
        amount: Property<f64>,
        #[serde(default)]
        join: LineJoin,
    },
    /// パンク・膨張（パーセント。負でパンク）。
    PuckerBloat {
        amount: Property<f64>,
    },
    ZigZag {
        size: Property<f64>,
        ridges: Property<f64>,
        #[serde(default)]
        smooth: bool,
    },
    /// 波形: パスを法線方向に正弦波で揺らす。
    Wave {
        amplitude: Property<f64>,
        wavelength: Property<f64>,
        /// 度。アニメートすると波が流れる。
        #[serde(default = "d0")]
        phase: Property<f64>,
    },
    WigglePath {
        size: Property<f64>,
        /// セグメントの細かさ（px あたりではなく、頂点間隔 px）。
        detail: Property<f64>,
        /// 1 秒あたりの揺れ回数。
        #[serde(default = "d0")]
        speed: Property<f64>,
        #[serde(default)]
        seed: u32,
    },
    RoundCorners {
        radius: Property<f64>,
    },
    Merge {
        #[serde(default)]
        mode: MergeMode,
    },
    Twist {
        /// 度。
        angle: Property<f64>,
        #[serde(default = "d00")]
        center: Property<[f64; 2]>,
    },
}

fn four() -> f64 {
    4.0
}

impl ShapeItem {
    pub fn fill(rgba: [f64; 4]) -> Self {
        ShapeItem::Fill { color: Property::constant(rgba), opacity: d100(), rule: FillRule::NonZero, gradient: None }
    }

    pub fn stroke(rgba: [f64; 4], width: f64) -> Self {
        ShapeItem::Stroke {
            color: Property::constant(rgba),
            opacity: d100(),
            width: p1(width),
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 4.0,
            dashes: Vec::new(),
            dash_offset: d0(),
            gradient: None,
        }
    }

    pub fn rect(w: f64, h: f64) -> Self {
        ShapeItem::Rect { size: p2(w, h), position: d00(), roundness: d0() }
    }

    pub fn ellipse(w: f64, h: f64) -> Self {
        ShapeItem::Ellipse { size: p2(w, h), position: d00() }
    }

    pub fn star(points: f64, outer: f64, inner: f64) -> Self {
        ShapeItem::Polystar {
            star: true,
            points: p1(points),
            position: d00(),
            rotation: d0(),
            outer_radius: p1(outer),
            inner_radius: p1(inner),
        }
    }

    pub fn polygon(points: f64, radius: f64) -> Self {
        ShapeItem::Polystar {
            star: false,
            points: p1(points),
            position: d00(),
            rotation: d0(),
            outer_radius: p1(radius),
            inner_radius: d0(),
        }
    }

    pub fn path(path: BezPath) -> Self {
        ShapeItem::Path { path: Property::constant(path) }
    }

    pub fn group(items: Vec<ShapeItem>) -> Self {
        ShapeItem::Group { name: String::new(), items, transform: ShapeTransform::default() }
    }
}

// ---------- マスク ----------

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMode {
    None,
    #[default]
    Add,
    Subtract,
    Intersect,
    Lighten,
    Darken,
    Difference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    #[serde(default)]
    pub name: String,
    /// レイヤー座標。
    pub path: Property<BezPath>,
    #[serde(default)]
    pub mode: MaskMode,
    #[serde(default)]
    pub inverted: bool,
    /// ぼかし（px、縦横）。
    #[serde(default = "d00")]
    pub feather: Property<[f64; 2]>,
    #[serde(default = "d100")]
    pub opacity: Property<f64>,
    /// 拡張（px。負で収縮）。
    #[serde(default = "d0")]
    pub expansion: Property<f64>,
    /// 可変幅フェザー: 頂点ごとの追加ぼかし半径（px）。空なら一様。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vertex_feather: Vec<f64>,
}

impl Mask {
    pub fn new(path: BezPath) -> Self {
        Mask {
            name: String::new(),
            path: Property::constant(path),
            mode: MaskMode::Add,
            inverted: false,
            feather: d00(),
            opacity: d100(),
            expansion: d0(),
            vertex_feather: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::property::Keyframe;
    use crate::time::Time;

    #[test]
    fn path_keyframes_interpolate_vertexwise() {
        let a = BezPath::rect(0.0, 0.0, 10.0, 10.0);
        let b = BezPath::rect(10.0, 0.0, 30.0, 10.0);
        let p = Property::constant(a.clone())
            .with_keys(vec![Keyframe::linear(Time::ZERO, a.clone()), Keyframe::linear(Time::from_seconds(1.0), b)]);
        let mid = p.eval(Time::from_seconds(0.5));
        assert_eq!(mid.vertices[0].p, [5.0, 0.0]);
        assert_eq!(mid.vertices[1].p, [25.0, 0.0]);
        assert!(mid.closed);
        // 頂点数が違うキー同士は補間せず停止
        let tri = BezPath::polygon(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        let p = Property::constant(a.clone())
            .with_keys(vec![Keyframe::linear(Time::ZERO, a.clone()), Keyframe::linear(Time::from_seconds(1.0), tri)]);
        assert_eq!(p.eval(Time::from_seconds(0.5)), a);
    }

    #[test]
    fn vertex_json_is_compact() {
        let json = serde_json::to_string(&PathVertex::corner(1.0, 2.0)).unwrap();
        assert_eq!(json, "[1.0,2.0,0.0,0.0,0.0,0.0]");
    }
}
