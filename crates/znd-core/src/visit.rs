//! レイヤーが持つ全プロパティの走査。エクスプレッションの解決・参照や、一括処理が使う。
//!
//! 各プロパティには安定したパス文字列が付く:
//! `transform.position`、`fx.0.radius`、`mask.1.feather`、`text.0.opacity`、`shape.0.2.size` など。

use crate::effect::ParamValue;
use crate::model::{Layer, LayerKind};
use crate::property::Property;
use crate::shape::{BezPath, Gradient, Mask, ShapeItem, ShapeTransform};
use crate::text::{TextAnimator, TextSelector};

pub enum PropRef<'a> {
    F1(&'a Property<f64>),
    F2(&'a Property<[f64; 2]>),
    F3(&'a Property<[f64; 3]>),
    F4(&'a Property<[f64; 4]>),
    Path(&'a Property<BezPath>),
}

pub enum PropMut<'a> {
    F1(&'a mut Property<f64>),
    F2(&'a mut Property<[f64; 2]>),
    F3(&'a mut Property<[f64; 3]>),
    F4(&'a mut Property<[f64; 4]>),
    Path(&'a mut Property<BezPath>),
}

impl PropRef<'_> {
    pub fn has_expression(&self) -> bool {
        let e = match self {
            PropRef::F1(p) => &p.expression,
            PropRef::F2(p) => &p.expression,
            PropRef::F3(p) => &p.expression,
            PropRef::F4(p) => &p.expression,
            PropRef::Path(p) => &p.expression,
        };
        e.as_ref().is_some_and(|e| e.enabled)
    }

    pub fn expression(&self) -> Option<&str> {
        let e = match self {
            PropRef::F1(p) => &p.expression,
            PropRef::F2(p) => &p.expression,
            PropRef::F3(p) => &p.expression,
            PropRef::F4(p) => &p.expression,
            PropRef::Path(p) => &p.expression,
        };
        e.as_ref().filter(|e| e.enabled).map(|e| e.source.as_str())
    }

    /// キーフレームだけで決まる値（エクスプレッション適用前）。パスは対象外。
    pub fn eval(&self, t: crate::time::Time) -> Option<Vec<f64>> {
        match self {
            PropRef::F1(p) => Some(vec![p.eval(t)]),
            PropRef::F2(p) => Some(p.eval(t).to_vec()),
            PropRef::F3(p) => Some(p.eval(t).to_vec()),
            PropRef::F4(p) => Some(p.eval(t).to_vec()),
            PropRef::Path(_) => None,
        }
    }

    pub fn key_times(&self) -> Vec<crate::time::Time> {
        match self {
            PropRef::F1(p) => p.keyframes.iter().map(|k| k.time).collect(),
            PropRef::F2(p) => p.keyframes.iter().map(|k| k.time).collect(),
            PropRef::F3(p) => p.keyframes.iter().map(|k| k.time).collect(),
            PropRef::F4(p) => p.keyframes.iter().map(|k| k.time).collect(),
            PropRef::Path(p) => p.keyframes.iter().map(|k| k.time).collect(),
        }
    }

    pub fn dims(&self) -> usize {
        match self {
            PropRef::F1(_) => 1,
            PropRef::F2(_) => 2,
            PropRef::F3(_) => 3,
            PropRef::F4(_) => 4,
            PropRef::Path(_) => 0,
        }
    }
}

impl PropMut<'_> {
    /// 値を定数で置き換える（キーフレームとエクスプレッションを外す）。次元が合わなければ false。
    pub fn set_constant(&mut self, v: &[f64]) -> bool {
        fn set<const N: usize>(p: &mut Property<[f64; N]>, v: &[f64]) -> bool {
            if v.len() < N {
                return false;
            }
            let mut a = [0.0; N];
            a.copy_from_slice(&v[..N]);
            p.value = a;
            p.keyframes.clear();
            p.expression = None;
            true
        }
        match self {
            PropMut::F1(p) => {
                let Some(x) = v.first() else { return false };
                p.value = *x;
                p.keyframes.clear();
                p.expression = None;
                true
            }
            PropMut::F2(p) => set(p, v),
            PropMut::F3(p) => set(p, v),
            PropMut::F4(p) => set(p, v),
            PropMut::Path(_) => false,
        }
    }
}

// 不変版と可変版で同じ走査を 2 回書かないよう、参照の種類だけを差し替えて生成する。
macro_rules! visitor {
    ($visit:ident, $shape:ident, $shape_tr:ident, $gradient:ident, $Ref:ident, $($m:tt)?) => {
        fn $shape_tr<'a>(tr: &'a $($m)? ShapeTransform, prefix: &str, f: &mut dyn FnMut(&str, $Ref<'a>)) {
            f(&format!("{prefix}.anchor"), $Ref::F2(&$($m)? tr.anchor));
            f(&format!("{prefix}.position"), $Ref::F2(&$($m)? tr.position));
            f(&format!("{prefix}.scale"), $Ref::F2(&$($m)? tr.scale));
            f(&format!("{prefix}.rotation"), $Ref::F1(&$($m)? tr.rotation));
            f(&format!("{prefix}.opacity"), $Ref::F1(&$($m)? tr.opacity));
        }

        fn $gradient<'a>(g: &'a $($m)? Option<Gradient>, prefix: &str, f: &mut dyn FnMut(&str, $Ref<'a>)) {
            if let Some(g) = g {
                f(&format!("{prefix}.gradient_start"), $Ref::F2(&$($m)? g.start));
                f(&format!("{prefix}.gradient_end"), $Ref::F2(&$($m)? g.end));
            }
        }

        fn $shape<'a>(items: &'a $($m)? Vec<ShapeItem>, prefix: &str, f: &mut dyn FnMut(&str, $Ref<'a>)) {
            for (i, item) in items.into_iter().enumerate() {
                let p = format!("{prefix}.{i}");
                match item {
                    ShapeItem::Group { items, transform, .. } => {
                        $shape_tr(transform, &format!("{p}.transform"), f);
                        $shape(items, &p, f);
                    }
                    ShapeItem::Rect { size, position, roundness } => {
                        f(&format!("{p}.size"), $Ref::F2(size));
                        f(&format!("{p}.position"), $Ref::F2(position));
                        f(&format!("{p}.roundness"), $Ref::F1(roundness));
                    }
                    ShapeItem::Ellipse { size, position } => {
                        f(&format!("{p}.size"), $Ref::F2(size));
                        f(&format!("{p}.position"), $Ref::F2(position));
                    }
                    ShapeItem::Polystar { points, position, rotation, outer_radius, inner_radius, .. } => {
                        f(&format!("{p}.points"), $Ref::F1(points));
                        f(&format!("{p}.position"), $Ref::F2(position));
                        f(&format!("{p}.rotation"), $Ref::F1(rotation));
                        f(&format!("{p}.outer_radius"), $Ref::F1(outer_radius));
                        f(&format!("{p}.inner_radius"), $Ref::F1(inner_radius));
                    }
                    ShapeItem::Path { path } => f(&format!("{p}.path"), $Ref::Path(path)),
                    ShapeItem::Fill { color, opacity, gradient, .. } => {
                        f(&format!("{p}.color"), $Ref::F4(color));
                        f(&format!("{p}.opacity"), $Ref::F1(opacity));
                        $gradient(gradient, &p, f);
                    }
                    ShapeItem::Stroke { color, opacity, width, dash_offset, gradient, .. } => {
                        f(&format!("{p}.color"), $Ref::F4(color));
                        f(&format!("{p}.opacity"), $Ref::F1(opacity));
                        f(&format!("{p}.width"), $Ref::F1(width));
                        f(&format!("{p}.dash_offset"), $Ref::F1(dash_offset));
                        $gradient(gradient, &p, f);
                    }
                    ShapeItem::Trim { start, end, offset } => {
                        f(&format!("{p}.start"), $Ref::F1(start));
                        f(&format!("{p}.end"), $Ref::F1(end));
                        f(&format!("{p}.offset"), $Ref::F1(offset));
                    }
                    ShapeItem::Repeater { copies, offset, transform, end_opacity } => {
                        f(&format!("{p}.copies"), $Ref::F1(copies));
                        f(&format!("{p}.offset"), $Ref::F1(offset));
                        f(&format!("{p}.end_opacity"), $Ref::F1(end_opacity));
                        $shape_tr(transform, &format!("{p}.transform"), f);
                    }
                    ShapeItem::OffsetPath { amount, .. } | ShapeItem::PuckerBloat { amount } => f(&format!("{p}.amount"), $Ref::F1(amount)),
                    ShapeItem::ZigZag { size, ridges, .. } => {
                        f(&format!("{p}.size"), $Ref::F1(size));
                        f(&format!("{p}.ridges"), $Ref::F1(ridges));
                    }
                    ShapeItem::Wave { amplitude, wavelength, phase } => {
                        f(&format!("{p}.amplitude"), $Ref::F1(amplitude));
                        f(&format!("{p}.wavelength"), $Ref::F1(wavelength));
                        f(&format!("{p}.phase"), $Ref::F1(phase));
                    }
                    ShapeItem::WigglePath { size, detail, speed, .. } => {
                        f(&format!("{p}.size"), $Ref::F1(size));
                        f(&format!("{p}.detail"), $Ref::F1(detail));
                        f(&format!("{p}.speed"), $Ref::F1(speed));
                    }
                    ShapeItem::RoundCorners { radius } => f(&format!("{p}.radius"), $Ref::F1(radius)),
                    ShapeItem::Twist { angle, center } => {
                        f(&format!("{p}.angle"), $Ref::F1(angle));
                        f(&format!("{p}.center"), $Ref::F2(center));
                    }
                    ShapeItem::Merge { .. } => {}
                }
            }
        }

        /// レイヤーの全プロパティを (パス, 参照) で列挙する。
        pub fn $visit<'a>(layer: &'a $($m)? Layer, f: &mut dyn FnMut(&str, $Ref<'a>)) {
            let tr = &$($m)? layer.transform;
            f("transform.anchor", $Ref::F2(&$($m)? tr.anchor));
            f("transform.position", $Ref::F2(&$($m)? tr.position));
            f("transform.scale", $Ref::F2(&$($m)? tr.scale));
            f("transform.rotation", $Ref::F1(&$($m)? tr.rotation));
            f("transform.opacity", $Ref::F1(&$($m)? tr.opacity));
            f("transform.position_z", $Ref::F1(&$($m)? tr.position_z));
            f("transform.anchor_z", $Ref::F1(&$($m)? tr.anchor_z));
            f("transform.rotation_x", $Ref::F1(&$($m)? tr.rotation_x));
            f("transform.rotation_y", $Ref::F1(&$($m)? tr.rotation_y));
            if let Some(p) = &$($m)? layer.time_remap {
                f("time_remap", $Ref::F1(p));
            }
            f("audio.level", $Ref::F1(&$($m)? layer.audio.level_db));

            let masks: &$($m)? Vec<Mask> = &$($m)? layer.masks;
            for (i, mask) in masks.into_iter().enumerate() {
                f(&format!("mask.{i}.path"), $Ref::Path(&$($m)? mask.path));
                f(&format!("mask.{i}.feather"), $Ref::F2(&$($m)? mask.feather));
                f(&format!("mask.{i}.opacity"), $Ref::F1(&$($m)? mask.opacity));
                f(&format!("mask.{i}.expansion"), $Ref::F1(&$($m)? mask.expansion));
            }

            for (i, fx) in (&$($m)? layer.effects).into_iter().enumerate() {
                for (name, value) in &$($m)? fx.params {
                    let path = format!("fx.{i}.{name}");
                    match value {
                        ParamValue::Scalar(p) => f(&path, $Ref::F1(p)),
                        ParamValue::Point(p) => f(&path, $Ref::F2(p)),
                        ParamValue::Color(p) => f(&path, $Ref::F4(p)),
                        _ => {}
                    }
                }
            }

            match &$($m)? layer.kind {
                LayerKind::Shape { items } => $shape(items, "shape", f),
                LayerKind::Text { animators, path, .. } => {
                    if let Some(p) = path {
                        f("text.path.first_margin", $Ref::F1(&$($m)? p.first_margin));
                    }
                    let animators: &$($m)? Vec<TextAnimator> = animators;
                    for (i, a) in animators.into_iter().enumerate() {
                        let pre = format!("text.{i}");
                        let pr = &$($m)? a.props;
                        if let Some(p) = &$($m)? pr.anchor { f(&format!("{pre}.anchor"), $Ref::F2(p)); }
                        if let Some(p) = &$($m)? pr.position { f(&format!("{pre}.position"), $Ref::F2(p)); }
                        if let Some(p) = &$($m)? pr.scale { f(&format!("{pre}.scale"), $Ref::F2(p)); }
                        if let Some(p) = &$($m)? pr.rotation { f(&format!("{pre}.rotation"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.opacity { f(&format!("{pre}.opacity"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.tracking { f(&format!("{pre}.tracking"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.fill_color { f(&format!("{pre}.fill_color"), $Ref::F4(p)); }
                        if let Some(p) = &$($m)? pr.stroke_color { f(&format!("{pre}.stroke_color"), $Ref::F4(p)); }
                        if let Some(p) = &$($m)? pr.stroke_width { f(&format!("{pre}.stroke_width"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.blur { f(&format!("{pre}.blur"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.character_offset { f(&format!("{pre}.character_offset"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.position_z { f(&format!("{pre}.position_z"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.rotation_x { f(&format!("{pre}.rotation_x"), $Ref::F1(p)); }
                        if let Some(p) = &$($m)? pr.rotation_y { f(&format!("{pre}.rotation_y"), $Ref::F1(p)); }
                        for (j, sel) in (&$($m)? a.selectors).into_iter().enumerate() {
                            let sp = format!("{pre}.sel.{j}");
                            match sel {
                                TextSelector::Range { start, end, offset, amount, .. } => {
                                    f(&format!("{sp}.start"), $Ref::F1(start));
                                    f(&format!("{sp}.end"), $Ref::F1(end));
                                    f(&format!("{sp}.offset"), $Ref::F1(offset));
                                    f(&format!("{sp}.amount"), $Ref::F1(amount));
                                }
                                TextSelector::Wiggly { max_amount, min_amount, wiggles_per_second, correlation, .. } => {
                                    f(&format!("{sp}.max_amount"), $Ref::F1(max_amount));
                                    f(&format!("{sp}.min_amount"), $Ref::F1(min_amount));
                                    f(&format!("{sp}.wiggles_per_second"), $Ref::F1(wiggles_per_second));
                                    f(&format!("{sp}.correlation"), $Ref::F1(correlation));
                                }
                                TextSelector::Expression { .. } => {}
                            }
                        }
                    }
                }
                LayerKind::Camera { zoom, point_of_interest, focus_distance, aperture, .. } => {
                    f("camera.zoom", $Ref::F1(zoom));
                    if let Some(p) = point_of_interest {
                        f("camera.point_of_interest", $Ref::F3(p));
                    }
                    f("camera.focus_distance", $Ref::F1(focus_distance));
                    f("camera.aperture", $Ref::F1(aperture));
                }
                LayerKind::Light { intensity, cone_angle, cone_feather, .. } => {
                    f("light.intensity", $Ref::F1(intensity));
                    f("light.cone_angle", $Ref::F1(cone_angle));
                    f("light.cone_feather", $Ref::F1(cone_feather));
                }
                _ => {}
            }
        }
    };
}

visitor!(visit_layer, visit_shape, visit_shape_tr, visit_gradient, PropRef,);
visitor!(visit_layer_mut, visit_shape_mut, visit_shape_tr_mut, visit_gradient_mut, PropMut, mut);

/// パスでプロパティを探す。
pub fn find_prop<'a>(layer: &'a Layer, path: &str) -> Option<PropRef<'a>> {
    let mut found = None;
    visit_layer(layer, &mut |p, r| {
        if found.is_none() && p == path {
            found = Some(r);
        }
    });
    found
}

/// レイヤーに有効なエクスプレッションが 1 つでもあるか。
pub fn has_expressions(layer: &Layer) -> bool {
    let mut any = false;
    visit_layer(layer, &mut |_, r| any |= r.has_expression());
    any || matches!(&layer.kind, LayerKind::Text { animators, .. } if animators.iter().any(|a| a.selectors.iter().any(|s| matches!(s, TextSelector::Expression { .. }))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_project;
    use crate::effect::EffectInstance;
    use crate::property::Expression;
    use crate::time::Time;

    #[test]
    fn visits_transform_effects_masks_and_shapes() {
        let mut p = demo_project();
        let layer = &mut p.compositions[0].layers[0];
        layer.effects.push(EffectInstance::new("gaussian_blur", 100.0, 100.0).unwrap());
        layer.masks.push(Mask::new(BezPath::rect(0.0, 0.0, 1.0, 1.0)));
        let mut paths = Vec::new();
        visit_layer(layer, &mut |p, _| paths.push(p.to_string()));
        for expect in ["transform.position", "transform.rotation_y", "fx.0.radius", "mask.0.feather", "mask.0.path", "audio.level"] {
            assert!(paths.iter().any(|p| p == expect), "{expect} が無い: {paths:?}");
        }
        assert!(find_prop(layer, "fx.0.radius").is_some_and(|r| r.eval(Time::ZERO) == Some(vec![10.0])));
        assert!(find_prop(layer, "nope").is_none());
    }

    #[test]
    fn mutable_visit_can_bake_constants() {
        let mut p = demo_project();
        let layer = &mut p.compositions[0].layers[6];
        assert!(!has_expressions(layer));
        layer.transform.rotation.expression = Some(Expression { source: "time * 90".into(), enabled: true });
        assert!(has_expressions(layer));
        visit_layer_mut(layer, &mut |path, mut m| {
            if path == "transform.rotation" {
                assert!(m.set_constant(&[45.0]));
            }
            if path == "transform.position" {
                assert!(!m.set_constant(&[1.0]), "次元不足は拒否");
                assert!(m.set_constant(&[1.0, 2.0]));
            }
        });
        assert_eq!(layer.transform.rotation.eval(Time::from_seconds(3.0)), 45.0);
        assert!(layer.transform.rotation.expression.is_none());
        assert_eq!(layer.transform.position.eval(Time::from_seconds(3.0)), [1.0, 2.0]);
        assert!(layer.transform.position.spatial, "空間プロパティの印は保つ");
        assert!(!has_expressions(layer));
    }
}
