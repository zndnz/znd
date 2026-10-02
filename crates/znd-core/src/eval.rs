//! 時刻 t におけるレイヤーのトランスフォーム評価。

use crate::model::{Composition, Layer};
use crate::time::Time;

/// 2D アフィン変換。`x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`。
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub fn translate(x: f64, y: f64) -> Affine {
        Affine { tx: x, ty: y, ..Self::IDENTITY }
    }

    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine { a: sx, d: sy, ..Self::IDENTITY }
    }

    /// 時計回り（Y 下向き座標系）に `deg` 度回転。
    pub fn rotate(deg: f64) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine { a: c, b: s, c: -s, d: c, tx: 0.0, ty: 0.0 }
    }

    /// `self ∘ o`（先に `o`、次に `self` を適用）。
    pub fn then_after(&self, o: &Affine) -> Affine {
        Affine {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            tx: self.a * o.tx + self.c * o.ty + self.tx,
            ty: self.b * o.tx + self.d * o.ty + self.ty,
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty)
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Affine { a, b, c, d, tx: -(a * self.tx + c * self.ty), ty: -(b * self.tx + d * self.ty) })
    }
}

/// レイヤー座標 → 親（またはコンポ）座標。
pub fn local_matrix(layer: &Layer, t: Time) -> Affine {
    let tr = &layer.transform;
    let anchor = tr.anchor.eval(t);
    let pos = tr.position.eval(t);
    let scale = tr.scale.eval(t);
    let rot = tr.rotation.eval(t);
    Affine::translate(pos[0], pos[1])
        .then_after(&Affine::rotate(rot))
        .then_after(&Affine::scale(scale[0] / 100.0, scale[1] / 100.0))
        .then_after(&Affine::translate(-anchor[0], -anchor[1]))
}

/// レイヤー座標 → コンポ座標。親チェーンをたどる（不透明度は継承しない）。
pub fn world_matrix(comp: &Composition, layer: &Layer, t: Time) -> Affine {
    let mut m = local_matrix(layer, t);
    let mut cur = layer.parent.as_ref();
    // 循環参照があっても止まるよう、レイヤー数で打ち切る。
    for _ in 0..comp.layers.len() {
        let Some(parent) = cur.and_then(|id| comp.layer(id)) else {
            break;
        };
        m = local_matrix(parent, t).then_after(&m);
        cur = parent.parent.as_ref();
    }
    m
}

/// コンポ時刻 `t` でレイヤーが表示するソース時刻（秒）。コマ打ち → タイムリマップ／再生速度の順に適用する。
pub fn layer_source_seconds(comp: &Composition, layer: &Layer, t: Time) -> f64 {
    let fr = comp.frame_rate;
    let t = if layer.hold_frames > 1 {
        // イン点を起点に N フレームごとに保持する
        let (f, f0) = (fr.time_to_frame(t), fr.time_to_frame(layer.in_point));
        let n = layer.hold_frames as i64;
        fr.frame_to_time(f0 + (f - f0).div_euclid(n) * n)
    } else {
        t
    };
    match &layer.time_remap {
        Some(remap) => remap.eval(t),
        None => (t - layer.start_time).seconds() * layer.speed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LayerId, LayerKind, Transform};
    use crate::property::Property;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn source_time_mapping() {
        use crate::property::Keyframe;
        let d = Time::from_seconds(10.0);
        let mut comp = Composition::new("c", "c", 100, 100, crate::time::FrameRate::FPS_24, d);
        let mut l = Layer::new("l", "l", LayerKind::Null, Transform::centered(0.0, 0.0, 0.0, 0.0), d);
        l.start_time = Time::from_seconds(1.0);
        l.in_point = Time::from_seconds(1.0);
        assert_eq!(layer_source_seconds(&comp, &l, Time::from_seconds(3.0)), 2.0);
        l.speed = 0.5;
        assert_eq!(layer_source_seconds(&comp, &l, Time::from_seconds(3.0)), 1.0);
        l.speed = 1.0;
        // 3 コマ打ち: イン点から 3 フレームごとに同じ時刻を返す
        l.hold_frames = 3;
        let fr = comp.frame_rate;
        let at = |f: i64| layer_source_seconds(&comp, &l, fr.frame_to_time(24 + f));
        assert_eq!(at(0), at(2));
        assert!((at(3) - 3.0 / 24.0).abs() < 1e-9);
        assert_eq!(at(3), at(5));
        // タイムリマップは開始時刻・速度より優先
        l.hold_frames = 1;
        l.time_remap = Some(Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 5.0), Keyframe::linear(d, 0.0)]));
        assert!((layer_source_seconds(&comp, &l, Time::from_seconds(4.0)) - 3.0).abs() < 1e-9);
        comp.layers.push(l);
    }

    #[test]
    fn inverse_roundtrip() {
        let m = Affine::translate(10.0, -5.0).then_after(&Affine::rotate(33.0)).then_after(&Affine::scale(2.0, 0.5));
        let inv = m.inverse().unwrap();
        let p = m.apply(3.0, 4.0);
        assert!(close(inv.apply(p.0, p.1), (3.0, 4.0)));
        assert!(Affine::scale(0.0, 1.0).inverse().is_none());
    }

    #[test]
    fn anchor_maps_to_position() {
        let mut tr = Transform::centered(100.0, 50.0, 1920.0, 1080.0);
        tr.rotation = Property::constant(90.0);
        tr.scale = Property::constant([200.0, 200.0]);
        let layer = Layer::new("a", "a", LayerKind::Null, tr, Time::from_seconds(1.0));
        let m = local_matrix(&layer, Time::ZERO);
        assert!(close(m.apply(50.0, 25.0), (960.0, 540.0)));
        // レイヤーの +X は 90° 回転でコンポの +Y へ、スケール 2 倍
        assert!(close(m.apply(60.0, 25.0), (960.0, 560.0)));
    }

    #[test]
    fn parent_chain_and_cycle_guard() {
        let d = Time::from_seconds(1.0);
        let mut parent = Layer::new("p", "p", LayerKind::Null, Transform::centered(0.0, 0.0, 200.0, 200.0), d);
        parent.transform.rotation = Property::constant(90.0);
        let mut child = Layer::new("c", "c", LayerKind::Null, Transform::centered(0.0, 0.0, 0.0, 0.0), d);
        child.transform.position = Property::spatial([10.0, 0.0]);
        child.parent = Some(LayerId::new("p"));
        let mut comp = Composition {
            id: crate::model::CompId::new("c"),
            name: "c".into(),
            width: 200,
            height: 200,
            pixel_aspect: 1.0,
            frame_rate: crate::time::FrameRate::FPS_30,
            duration: d,
            background: [0.0; 3],
            layers: vec![child, parent],
            markers: vec![],
            ..Composition::blank()
        };
        let m = world_matrix(&comp, &comp.layers[0], Time::ZERO);
        assert!(close(m.apply(0.0, 0.0), (100.0, 110.0)));
        // 循環させても無限ループしない
        comp.layers[1].parent = Some(LayerId::new("c"));
        let _ = world_matrix(&comp, &comp.layers[0], Time::ZERO);
    }
}
