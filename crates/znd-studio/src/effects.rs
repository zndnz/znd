//! エフェクトの一覧（組み込みと読み込み済みのプラグイン）を、画面で扱いやすい形にする。

use serde::Serialize;
use znd_core::effect::{all_effects, Pd};

#[derive(Serialize)]
pub struct EffectInfo {
    kind: String,
    label: String,
    category: String,
    params: Vec<ParamInfo>,
}

#[derive(Serialize)]
pub struct ParamInfo {
    name: String,
    label: String,
    /// "float" / "point" / "color" / "bool" / "choice" / "layer" / "text" / "curve"
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    choices: Vec<String>,
}

pub fn catalog() -> Vec<EffectInfo> {
    all_effects()
        .into_iter()
        .map(|d| EffectInfo {
            kind: d.kind.to_string(),
            label: d.label.to_string(),
            category: d.category.to_string(),
            params: d
                .params
                .iter()
                .map(|p| {
                    let (kind, min, max, choices) = match &p.kind {
                        Pd::F(_, lo, hi) => ("float", Some(*lo), Some(*hi), Vec::new()),
                        Pd::P(..) => ("point", None, None, Vec::new()),
                        Pd::C(_) => ("color", None, None, Vec::new()),
                        Pd::B(_) => ("bool", None, None, Vec::new()),
                        Pd::I(_, labels) => ("choice", None, None, labels.iter().map(|s| s.to_string()).collect()),
                        Pd::L => ("layer", None, None, Vec::new()),
                        Pd::S => ("text", None, None, Vec::new()),
                        Pd::Curve(_) => ("curve", None, None, Vec::new()),
                    };
                    ParamInfo { name: p.name.to_string(), label: p.label.to_string(), kind, min, max, choices }
                })
                .collect(),
        })
        .collect()
}
