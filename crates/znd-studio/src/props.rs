//! プロパティとキーフレームの計算。どれも「レイヤーを受け取り、書き換えたレイヤーを返す」純粋な関数で、
//! 画面（TypeScript）が結果をプロジェクトに戻して `commit` する。取り消しの単位は画面が決める。
//!
//! プロパティは `znd_core::visit` のパス（`transform.position`・`fx.0.radius`・`mask.1.feather`・`text.0.opacity`
//! など）で指すので、トランスフォームだけでなく、エフェクト・マスク・シェイプ・テキストアニメーター・カメラ・ライトの
//! 値もすべて同じやり方でキーフレームにできる。

use serde::{Deserialize, Serialize};
use znd_core::ease::{EaseCurve, CURVE_PRESETS};
use znd_core::keys::{self, KeyAddr};
use znd_core::props::{apply_edit, collect_rows, path_label, KeyKind, PropEdit};
use znd_core::*;

/// 画面に出すプロパティ 1 つ。
#[derive(Serialize)]
pub struct PropInfo {
    path: String,
    label: String,
    /// その時刻の値（成分ごと）。パス（マスクの形）は空。
    vals: Vec<f64>,
    is_path: bool,
    animated: bool,
    expr: Option<String>,
    keys: Vec<KeyInfo>,
}

#[derive(Serialize)]
pub struct KeyInfo {
    time: Time,
    kind: KeyKind,
    /// このキーから次のキーまでの区間のイージングカーブ。
    curve: Option<EaseCurve>,
}

/// レイヤーの全プロパティ（`visit` の順）と、時刻 `t` での値。
pub fn list(layer: &Layer, t: Time) -> Vec<PropInfo> {
    collect_rows(layer, t)
        .into_iter()
        .map(|r| PropInfo {
            label: path_label(layer, &r.path),
            keys: r.keys.iter().zip(&r.curves).map(|(&(time, kind), &curve)| KeyInfo { time, kind, curve }).collect(),
            path: r.path,
            vals: r.vals,
            is_path: r.is_path,
            animated: r.animated,
            expr: r.expr,
        })
        .collect()
}

pub fn edit(mut layer: Layer, path: &str, t: Time, op: &PropEdit) -> Layer {
    apply_edit(&mut layer, path, t, op);
    layer
}

/// 選んだキーへの一括操作。
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum KeysOp {
    /// 区間にイージングカーブを付ける（`None` で外してハンドルに戻す）。
    Curve { curve: Option<EaseCurve> },
    /// 補間の種類（リニア・イージーイーズ・停止など）。
    Kind { kind: KeyKind },
    /// 時間方向に動かす。
    Shift { dt: Time },
    Delete,
}

/// 操作後のレイヤーと、操作後の選択。
#[derive(Serialize)]
pub struct KeysResult {
    layer: Layer,
    keys: Vec<KeyAddr>,
}

pub fn keys_op(mut layer: Layer, sel: Vec<KeyAddr>, op: &KeysOp) -> KeysResult {
    let keys = match op {
        KeysOp::Curve { curve } => {
            keys::set_curve(&mut layer, &sel, *curve);
            sel
        }
        KeysOp::Kind { kind } => {
            keys::set_style(&mut layer, &sel, &kind.style());
            sel
        }
        KeysOp::Shift { dt } => keys::shift(&mut layer, &sel, *dt),
        KeysOp::Delete => {
            keys::delete(&mut layer, &sel);
            Vec::new()
        }
    };
    KeysResult { layer, keys }
}

/// 選んだキーの最初の区間のカーブ（スポイト）。
pub fn curve_of(layer: &Layer, sel: &[KeyAddr]) -> Option<EaseCurve> {
    keys::curve_of(layer, sel)
}

#[derive(Serialize)]
pub struct Preset {
    group: &'static str,
    name: &'static str,
    curve: EaseCurve,
    /// 形を描くための値（`SAMPLES + 1` 点）。
    samples: Vec<f64>,
}

const SAMPLES: usize = 48;

pub fn samples(curve: &EaseCurve, n: usize) -> Vec<f64> {
    let n = n.clamp(2, 512);
    (0..=n).map(|i| curve.eval(i as f64 / n as f64)).collect()
}

pub fn presets() -> Vec<Preset> {
    CURVE_PRESETS.iter().map(|p| Preset { group: p.group, name: p.name, curve: p.curve, samples: samples(&p.curve, SAMPLES) }).collect()
}

/// `cubic-bezier(...)` などの文字を読む。
pub fn parse_curve(text: &str) -> Option<EaseCurve> {
    EaseCurve::parse_css(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::ease::EaseDir;

    fn layer_with_blur() -> Layer {
        let mut l = Layer::new("l", "平面", LayerKind::Null, Transform::centered(0.0, 0.0, 100.0, 100.0), Time::from_seconds(5.0));
        l.effects.push(EffectInstance::new("gaussian_blur", 100.0, 100.0).unwrap());
        l
    }

    /// エフェクトのパラメータも、トランスフォームと同じ操作でキーフレームにできる。
    #[test]
    fn effect_params_are_listed_and_can_be_keyframed() {
        let l = layer_with_blur();
        let rows = list(&l, Time::ZERO);
        let fx: Vec<&PropInfo> = rows.iter().filter(|r| r.path.starts_with("fx.0.")).collect();
        assert!(!fx.is_empty(), "エフェクトのパラメータが一覧に無い");
        let path = fx[0].path.clone();
        assert!(!fx[0].label.is_empty() && fx[0].label != path, "パラメータに名前が付いていない");

        let t1 = Time::from_seconds(1.0);
        let l = edit(l, &path, Time::ZERO, &PropEdit::ToggleAnimation);
        let l = edit(l, &path, t1, &PropEdit::Set { value: vec![12.0] });
        let row = list(&l, t1).into_iter().find(|r| r.path == path).unwrap();
        assert!(row.animated);
        assert_eq!(row.keys.len(), 2);
        assert_eq!(row.vals, vec![12.0]);
    }

    /// 区間にイージングカーブを付け、拾い、外せる。
    #[test]
    fn curves_can_be_set_picked_and_removed() {
        let l = layer_with_blur();
        let l = edit(l, "transform.rotation", Time::ZERO, &PropEdit::ToggleAnimation);
        let t1 = Time::from_seconds(2.0);
        let l = edit(l, "transform.rotation", t1, &PropEdit::Set { value: vec![90.0] });
        let sel = vec![KeyAddr::new("transform.rotation", Time::ZERO)];
        let curve = EaseCurve::bounce(EaseDir::Out);
        let r = keys_op(l, sel.clone(), &KeysOp::Curve { curve: Some(curve) });
        assert_eq!(curve_of(&r.layer, &sel), Some(curve));
        let row = list(&r.layer, Time::ZERO).into_iter().find(|p| p.path == "transform.rotation").unwrap();
        assert_eq!(row.keys[0].curve, Some(curve));

        let r = keys_op(r.layer, sel.clone(), &KeysOp::Curve { curve: None });
        assert_eq!(curve_of(&r.layer, &sel), None);
    }

    /// 選んだキーを動かすと、動かした後の選択が返る。
    #[test]
    fn shifting_keys_returns_the_new_selection() {
        let l = edit(layer_with_blur(), "transform.opacity", Time::ZERO, &PropEdit::ToggleAnimation);
        let dt = Time::from_seconds(0.5);
        let r = keys_op(l, vec![KeyAddr::new("transform.opacity", Time::ZERO)], &KeysOp::Shift { dt });
        assert_eq!(r.keys, vec![KeyAddr::new("transform.opacity", dt)]);
        let row = list(&r.layer, Time::ZERO).into_iter().find(|p| p.path == "transform.opacity").unwrap();
        assert_eq!(row.keys[0].time, dt);
    }

    #[test]
    fn presets_have_samples_from_0_to_1() {
        for p in presets() {
            assert_eq!(p.samples.first(), Some(&0.0), "{}", p.name);
            assert_eq!(p.samples.last(), Some(&1.0), "{}", p.name);
        }
    }
}
