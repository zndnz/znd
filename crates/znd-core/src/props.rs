//! プロパティの一覧と、プロパティへの操作（値の設定・キーフレーム・補間・エクスプレッション）。
//! どの画面（egui の znd-app、Tauri の znd-studio）からも同じ操作で使う。

use crate::effect::effect_def;
use crate::visit::{visit_layer, visit_layer_mut, PropMut, PropRef};
use crate::*;
use serde::{Deserialize, Serialize};

/// キーフレームの補間種別（UI 上の選択肢）。
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    Linear,
    EasyEase,
    EaseIn,
    EaseOut,
    AutoBezier,
    Hold,
}

impl KeyKind {
    pub const ALL: [KeyKind; 6] =
        [KeyKind::Linear, KeyKind::EasyEase, KeyKind::EaseIn, KeyKind::EaseOut, KeyKind::AutoBezier, KeyKind::Hold];

    pub fn label(self) -> &'static str {
        match self {
            KeyKind::Linear => "リニア",
            KeyKind::EasyEase => "イージーイーズ",
            KeyKind::EaseIn => "イーズイン",
            KeyKind::EaseOut => "イーズアウト",
            KeyKind::AutoBezier => "自動ベジェ",
            KeyKind::Hold => "停止",
        }
    }

    pub fn of<V>(k: &Keyframe<V>) -> KeyKind {
        match (k.interp_in, k.interp_out, k.bezier_mode) {
            (_, Interp::Hold, _) => KeyKind::Hold,
            (Interp::Bezier, Interp::Bezier, BezierMode::Auto) => KeyKind::AutoBezier,
            (Interp::Bezier, Interp::Bezier, _) => KeyKind::EasyEase,
            (Interp::Bezier, _, _) => KeyKind::EaseIn,
            (_, Interp::Bezier, _) => KeyKind::EaseOut,
            _ => KeyKind::Linear,
        }
    }

    pub fn apply<V>(self, k: &mut Keyframe<V>) {
        let (interp_in, interp_out, mode) = match self {
            KeyKind::Linear => (Interp::Linear, Interp::Linear, BezierMode::Manual),
            KeyKind::EasyEase => (Interp::Bezier, Interp::Bezier, BezierMode::Manual),
            // イーズイン = このキーに入ってくる側だけ減速する
            KeyKind::EaseIn => (Interp::Bezier, Interp::Linear, BezierMode::Manual),
            KeyKind::EaseOut => (Interp::Linear, Interp::Bezier, BezierMode::Manual),
            KeyKind::AutoBezier => (Interp::Bezier, Interp::Bezier, BezierMode::Auto),
            KeyKind::Hold => (k.interp_in, Interp::Hold, BezierMode::Manual),
        };
        k.interp_in = interp_in;
        k.interp_out = interp_out;
        k.bezier_mode = mode;
        k.ease_in.clear();
        k.ease_out.clear();
        // 補間をハンドルで決め直すので、区間のイージングカーブは外す
        k.curve = None;
    }
}

impl KeyKind {
    /// 一括変更用の指定にする。
    pub fn style(self) -> crate::keys::KeyStyle {
        let both = |i, o, m| crate::keys::KeyStyle {
            interp_in: Some(i),
            interp_out: Some(o),
            bezier_mode: Some(m),
            ease_in: None,
            ease_out: None,
        };
        match self {
            KeyKind::Linear => both(Interp::Linear, Interp::Linear, BezierMode::Manual),
            KeyKind::EasyEase => both(Interp::Bezier, Interp::Bezier, BezierMode::Manual),
            KeyKind::EaseIn => both(Interp::Bezier, Interp::Linear, BezierMode::Manual),
            KeyKind::EaseOut => both(Interp::Linear, Interp::Bezier, BezierMode::Manual),
            KeyKind::AutoBezier => both(Interp::Bezier, Interp::Bezier, BezierMode::Auto),
            KeyKind::Hold => crate::keys::KeyStyle {
                interp_out: Some(Interp::Hold),
                bezier_mode: Some(BezierMode::Manual),
                ..crate::keys::KeyStyle::new()
            },
        }
    }
}

/// プロパティへの操作。値は成分の配列で持つので、どの型のプロパティにも使える。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PropEdit {
    Set {
        value: Vec<f64>,
    },
    /// アニメーションの切り替え: 開始（現在値でキー）／解除（現在値で固定）。
    ToggleAnimation,
    AddKey,
    RemoveKey,
    Kind {
        kind: KeyKind,
    },
    /// エクスプレッションの設定（`None` で削除）。
    Expr {
        source: Option<String>,
    },
    /// 時刻 `from` のキーを `to` へ動かす（グラフエディター）。
    MoveKey {
        from: Time,
        to: Time,
    },
}

impl PropEdit {
    pub fn coalesce(&self) -> bool {
        matches!(self, PropEdit::Set { .. } | PropEdit::MoveKey { .. })
    }
}

fn apply_to<V: Animatable>(p: &mut Property<V>, t: Time, edit: &PropEdit) {
    match edit {
        PropEdit::Set { value: v } => {
            let cur = p.eval(t);
            if v.len() >= cur.dims() {
                p.set_value_at(t, cur.with_components(v));
            }
        }
        PropEdit::ToggleAnimation => {
            let v = p.eval(t);
            if p.is_animated() {
                p.keyframes.clear();
                p.value = v;
            } else {
                p.set_key(t, v);
            }
        }
        PropEdit::AddKey => {
            let v = p.eval(t);
            p.set_key(t, v);
        }
        PropEdit::RemoveKey => {
            p.remove_key(t);
        }
        PropEdit::Kind { kind } => {
            if let Some(i) = p.key_index_at(t) {
                kind.apply(&mut p.keyframes[i]);
            }
        }
        PropEdit::Expr { source: src } => p.expression = src.as_ref().map(|s| Expression { source: s.clone(), enabled: true }),
        PropEdit::MoveKey { from, to } => {
            if let Some(i) = p.key_index_at(*from) {
                if p.key_index_at(*to).is_none() {
                    p.keyframes[i].time = *to;
                    p.normalize_keys();
                }
            }
        }
    }
}

/// レイヤーの `path` のプロパティに操作を適用する。
pub fn apply_edit(layer: &mut Layer, path: &str, t: Time, edit: &PropEdit) {
    // タイムリマップは、無ければ作る（現在のソース時刻から始める）
    if path == "time_remap" && layer.time_remap.is_none() {
        layer.time_remap = Some(Property::constant((t - layer.start_time).seconds() * layer.speed));
    }
    visit_layer_mut(layer, &mut |p, m| {
        if p == path {
            match m {
                PropMut::F1(pr) => apply_to(pr, t, edit),
                PropMut::F2(pr) => apply_to(pr, t, edit),
                PropMut::F3(pr) => apply_to(pr, t, edit),
                PropMut::F4(pr) => apply_to(pr, t, edit),
                PropMut::Path(pr) => apply_to(pr, t, edit),
            }
        }
    });
}

/// 表示用に取り出したプロパティ 1 つぶんの状態。
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub path: String,
    pub vals: Vec<f64>,
    pub is_path: bool,
    pub animated: bool,
    pub on_key: Option<KeyKind>,
    pub expr: Option<String>,
    /// キーフレームの時刻と補間の種類（昇順）。
    pub keys: Vec<(Time, KeyKind)>,
    /// キーごとの、次のキーまでの区間のイージングカーブ（`keys` と同じ並び）。
    pub curves: Vec<Option<EaseCurve>>,
}

fn snapshot<V: Animatable>(path: &str, p: &Property<V>, t: Time, is_path: bool) -> Row {
    let v = p.eval(t);
    Row {
        path: path.to_string(),
        vals: if is_path { Vec::new() } else { (0..v.dims()).map(|d| v.component(d)).collect() },
        is_path,
        animated: p.is_animated(),
        on_key: p.key_index_at(t).map(|i| KeyKind::of(&p.keyframes[i])),
        expr: p.expression.as_ref().map(|e| e.source.clone()),
        keys: p.keyframes.iter().map(|k| (k.time, KeyKind::of(k))).collect(),
        curves: p.keyframes.iter().map(|k| k.curve).collect(),
    }
}

pub fn collect_rows(layer: &Layer, t: Time) -> Vec<Row> {
    let mut rows = Vec::new();
    visit_layer(layer, &mut |path, r| {
        rows.push(match r {
            PropRef::F1(p) => snapshot(path, p, t, false),
            PropRef::F2(p) => snapshot(path, p, t, false),
            PropRef::F3(p) => snapshot(path, p, t, false),
            PropRef::F4(p) => snapshot(path, p, t, false),
            PropRef::Path(p) => snapshot(path, p, t, true),
        })
    });
    rows
}

/// プロパティのパスを、画面に出す名前にする。
pub fn path_label(layer: &Layer, path: &str) -> String {
    let fixed = match path {
        "transform.anchor" => "アンカーポイント",
        "transform.position" => "位置",
        "transform.scale" => "スケール",
        "transform.rotation" => "回転",
        "transform.opacity" => "不透明度",
        "transform.position_z" => "Z 位置",
        "transform.anchor_z" => "アンカー Z",
        "transform.rotation_x" => "X 回転",
        "transform.rotation_y" => "Y 回転",
        "time_remap" => "タイムリマップ（秒）",
        "audio.level" => "オーディオレベル（dB）",
        "camera.zoom" => "ズーム",
        "camera.point_of_interest" => "目標点",
        "camera.focus_distance" => "フォーカス距離",
        "camera.aperture" => "絞り",
        "light.intensity" => "強度（%）",
        "light.cone_angle" => "円錐角度",
        "light.cone_feather" => "円錐ぼかし",
        "text.path.first_margin" => "パス: 最初のマージン",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    let parts: Vec<&str> = path.split('.').collect();
    let word = |w: &str| -> String {
        match w {
            "path" => "パス",
            "feather" => "ぼかし",
            "opacity" => "不透明度",
            "expansion" => "拡張",
            "anchor" => "アンカー",
            "position" => "位置",
            "scale" => "スケール",
            "rotation" => "回転",
            "tracking" => "トラッキング",
            "fill_color" => "塗りの色",
            "stroke_color" => "線の色",
            "stroke_width" => "線幅",
            "blur" => "ブラー",
            "character_offset" => "文字オフセット",
            "position_z" => "Z 位置",
            "rotation_x" => "X 回転",
            "rotation_y" => "Y 回転",
            "start" => "開始",
            "end" => "終了",
            "offset" => "オフセット",
            "amount" => "量",
            "max_amount" => "最大量",
            "min_amount" => "最小量",
            "wiggles_per_second" => "ウィグル／秒",
            "correlation" => "相関",
            "size" => "サイズ",
            "roundness" => "角丸",
            "points" => "頂点数",
            "outer_radius" => "外半径",
            "inner_radius" => "内半径",
            "color" => "色",
            "width" => "線幅",
            "dash_offset" => "破線オフセット",
            "copies" => "コピー数",
            "end_opacity" => "終了不透明度",
            "ridges" => "折り返し",
            "amplitude" => "振幅",
            "wavelength" => "波長",
            "phase" => "位相",
            "detail" => "細かさ",
            "speed" => "速度",
            "radius" => "半径",
            "angle" => "角度",
            "center" => "中心",
            "transform" => "変形",
            "gradient_start" => "グラデーション開始",
            "gradient_end" => "グラデーション終了",
            other => other,
        }
        .to_string()
    };
    match parts.as_slice() {
        ["fx", i, name] => {
            let label = i
                .parse::<usize>()
                .ok()
                .and_then(|i| layer.effects.get(i))
                .and_then(|fx| effect_def(&fx.kind))
                .and_then(|d| d.params.iter().find(|p| p.name == *name))
                .map(|p| p.label);
            label.unwrap_or(name).to_string()
        }
        ["mask", _, name] => word(name),
        ["text", _, "sel", j, name] => format!("セレクター {}: {}", j.parse::<usize>().map(|v| v + 1).unwrap_or(0), word(name)),
        ["text", _, name] => word(name),
        ["shape", rest @ ..] => rest
            .iter()
            .map(|w| if w.parse::<usize>().is_ok() { format!("{}", w.parse::<usize>().unwrap() + 1) } else { word(w) })
            .collect::<Vec<_>>()
            .join(" › "),
        _ => path.to_string(),
    }
}
