//! インスペクタの細部: シェイプの要素の追加、テキストアニメーターの属性、エフェクトのプリセット、
//! 素材のプロキシ、フォントの有無、エクスプレッションのエラー、ファイルを選ぶ欄。

use crate::{lock, Shared, Snapshot};
use serde::{Deserialize, Serialize};
use tauri::State;
use znd_core::effect::EffectPreset;
use znd_core::shape::{self, ShapeItem};
use znd_core::text::{AnimatorProps, SelectorUnit, TextAnimator, TextSelector};
use znd_core::*;

/// シェイプに足せる要素（キー, 表示名）。
pub const SHAPE_ITEMS: [(&str, &str); 16] = [
    ("rect", "矩形"),
    ("ellipse", "楕円"),
    ("polygon", "多角形"),
    ("star", "スター"),
    ("fill", "塗り"),
    ("stroke", "線"),
    ("trim", "トリムパス"),
    ("repeater", "リピーター"),
    ("offset", "パスのオフセット"),
    ("pucker", "パンク・膨張"),
    ("zigzag", "ジグザグ"),
    ("wave", "波形"),
    ("wiggle", "ウィグルパス"),
    ("round", "角丸"),
    ("twist", "ねじれ"),
    ("merge", "パスの結合（中マド）"),
];

fn shape_item(key: &str) -> Option<ShapeItem> {
    let p = |v: f64| Property::constant(v);
    Some(match key {
        "rect" => ShapeItem::rect(200.0, 200.0),
        "ellipse" => ShapeItem::ellipse(200.0, 200.0),
        "polygon" => ShapeItem::polygon(6.0, 120.0),
        "star" => ShapeItem::star(5.0, 140.0, 60.0),
        "fill" => ShapeItem::fill([1.0, 1.0, 1.0, 1.0]),
        "stroke" => ShapeItem::stroke([1.0, 1.0, 1.0, 1.0], 6.0),
        "trim" => ShapeItem::Trim { start: p(0.0), end: p(100.0), offset: p(0.0) },
        "repeater" => ShapeItem::Repeater {
            copies: p(3.0),
            offset: p(0.0),
            transform: shape::ShapeTransform { position: Property::constant([100.0, 0.0]), ..Default::default() },
            end_opacity: p(100.0),
        },
        "offset" => ShapeItem::OffsetPath { amount: p(10.0), join: Default::default() },
        "pucker" => ShapeItem::PuckerBloat { amount: p(30.0) },
        "zigzag" => ShapeItem::ZigZag { size: p(10.0), ridges: p(5.0), smooth: false },
        "wave" => ShapeItem::Wave { amplitude: p(10.0), wavelength: p(60.0), phase: p(0.0) },
        "wiggle" => ShapeItem::WigglePath { size: p(10.0), detail: p(20.0), speed: p(2.0), seed: 0 },
        "round" => ShapeItem::RoundCorners { radius: p(20.0) },
        "twist" => ShapeItem::Twist { angle: p(60.0), center: Property::constant([0.0, 0.0]) },
        "merge" => ShapeItem::Merge { mode: shape::MergeMode::Exclude },
        _ => return None,
    })
}

/// アニメーターに付けられる属性（キー, 表示名）。足した直後は「変化なし」の値になる。
pub const ANIMATOR_PROPS: [(&str, &str); 13] = [
    ("position", "位置"),
    ("scale", "スケール"),
    ("rotation", "回転"),
    ("opacity", "不透明度"),
    ("anchor", "アンカーポイント"),
    ("tracking", "字間"),
    ("fill_color", "塗りの色"),
    ("stroke_color", "線の色"),
    ("stroke_width", "線の幅"),
    ("blur", "ブラー"),
    ("character_offset", "文字のずらし"),
    ("position_z", "Z 位置（3D）"),
    ("rotation_x", "X 回転（3D）"),
];

fn animator_prop_set(p: &AnimatorProps, key: &str) -> bool {
    match key {
        "position" => p.position.is_some(),
        "scale" => p.scale.is_some(),
        "rotation" => p.rotation.is_some(),
        "opacity" => p.opacity.is_some(),
        "anchor" => p.anchor.is_some(),
        "tracking" => p.tracking.is_some(),
        "fill_color" => p.fill_color.is_some(),
        "stroke_color" => p.stroke_color.is_some(),
        "stroke_width" => p.stroke_width.is_some(),
        "blur" => p.blur.is_some(),
        "character_offset" => p.character_offset.is_some(),
        "position_z" => p.position_z.is_some(),
        "rotation_x" => p.rotation_x.is_some(),
        _ => false,
    }
}

/// 属性を足す（変化なしの値で）・外す。
fn toggle_animator_prop(p: &mut AnimatorProps, key: &str, on: bool) {
    fn set<T: Animatable>(slot: &mut Option<Property<T>>, on: bool, neutral: T) {
        *slot = on.then(|| Property::constant(neutral));
    }
    match key {
        "position" => set(&mut p.position, on, [0.0, 0.0]),
        "scale" => set(&mut p.scale, on, [100.0, 100.0]),
        "rotation" => set(&mut p.rotation, on, 0.0),
        "opacity" => set(&mut p.opacity, on, 100.0),
        "anchor" => set(&mut p.anchor, on, [0.0, 0.0]),
        "tracking" => set(&mut p.tracking, on, 0.0),
        "fill_color" => set(&mut p.fill_color, on, [1.0, 1.0, 1.0, 1.0]),
        "stroke_color" => set(&mut p.stroke_color, on, [0.0, 0.0, 0.0, 1.0]),
        "stroke_width" => set(&mut p.stroke_width, on, 0.0),
        "blur" => set(&mut p.blur, on, 0.0),
        "character_offset" => set(&mut p.character_offset, on, 0.0),
        "position_z" => set(&mut p.position_z, on, 0.0),
        "rotation_x" => set(&mut p.rotation_x, on, 0.0),
        _ => {}
    }
}

#[derive(Serialize)]
pub struct AnimatorInfo {
    name: String,
    /// 付いている属性のキー。
    props: Vec<&'static str>,
    /// 最初の範囲セレクターの単位（"characters" など。無ければ null）。
    unit: Option<SelectorUnit>,
}

#[derive(Serialize)]
pub struct Catalog {
    shape_items: Vec<(&'static str, &'static str)>,
    animator_props: Vec<(&'static str, &'static str)>,
}

#[tauri::command]
pub fn inspector_catalog() -> Catalog {
    Catalog { shape_items: SHAPE_ITEMS.to_vec(), animator_props: ANIMATOR_PROPS.to_vec() }
}

/// テキストのレイヤーのアニメーター（属性と単位）。
#[tauri::command]
pub fn animators(layer: Layer) -> Vec<AnimatorInfo> {
    let LayerKind::Text { animators, .. } = &layer.kind else { return Vec::new() };
    animators
        .iter()
        .map(|a| AnimatorInfo {
            name: a.name.clone(),
            props: ANIMATOR_PROPS.iter().filter(|(k, _)| animator_prop_set(&a.props, k)).map(|(k, _)| *k).collect(),
            unit: match a.selectors.first() {
                Some(TextSelector::Range { unit, .. }) => Some(*unit),
                _ => None,
            },
        })
        .collect()
}

/// レイヤーの中身への操作（レイヤーを受け取り、書き換えたレイヤーを返す。画面が `commit` する）。
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ContentOp {
    /// シェイプに要素を足す（図形とオペレーターは塗り・線より前、塗り・線は末尾）。
    AddShapeItem { key: String },
    RemoveLastShapeItem,
    AddAnimator,
    RemoveAnimator { index: usize },
    AnimatorProp { index: usize, key: String, on: bool },
    AnimatorUnit { index: usize, unit: SelectorUnit },
}

#[tauri::command]
pub fn content_op(mut layer: Layer, op: ContentOp) -> Result<Layer, String> {
    match (&mut layer.kind, op) {
        (LayerKind::Shape { items }, ContentOp::AddShapeItem { key }) => {
            let item = shape_item(&key).ok_or("知らない要素です")?;
            let paint = matches!(item, ShapeItem::Fill { .. } | ShapeItem::Stroke { .. });
            let at = if paint { items.len() } else { items.iter().position(|i| matches!(i, ShapeItem::Fill { .. } | ShapeItem::Stroke { .. })).unwrap_or(items.len()) };
            items.insert(at, item);
        }
        (LayerKind::Shape { items }, ContentOp::RemoveLastShapeItem) => {
            items.pop();
        }
        (LayerKind::Text { animators, .. }, ContentOp::AddAnimator) => {
            let name = format!("アニメーター {}", animators.len() + 1);
            animators.push(TextAnimator { name, selectors: vec![TextSelector::range()], props: AnimatorProps::default() });
        }
        (LayerKind::Text { animators, .. }, ContentOp::RemoveAnimator { index }) if index < animators.len() => {
            animators.remove(index);
        }
        (LayerKind::Text { animators, .. }, ContentOp::AnimatorProp { index, key, on }) => {
            let a = animators.get_mut(index).ok_or("アニメーターがありません")?;
            toggle_animator_prop(&mut a.props, &key, on);
        }
        (LayerKind::Text { animators, .. }, ContentOp::AnimatorUnit { index, unit: u }) => {
            if let Some(TextSelector::Range { unit, .. }) = animators.get_mut(index).and_then(|a| a.selectors.first_mut()) {
                *unit = u;
            }
        }
        _ => return Err("このレイヤーにはできない操作です".into()),
    }
    Ok(layer)
}

// ---------- エフェクトのプリセット ----------

/// レイヤーのエフェクト（キーフレーム・エクスプレッション込み）をファイルに保存する。
#[tauri::command]
pub fn effect_preset_save(name: String, effects: Vec<EffectInstance>) -> Option<String> {
    let path = rfd::FileDialog::new().add_filter("エフェクトプリセット", &["json"]).set_file_name("preset.zndfx.json").save_file()?;
    let preset = EffectPreset { name, effects };
    Some(
        match serde_json::to_string_pretty(&preset).map_err(|e| e.to_string()).and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string())) {
            Ok(()) => format!("プリセットを {} に保存しました", path.display()),
            Err(e) => format!("保存できません: {e}"),
        },
    )
}

/// プリセットのファイルを選んで、そのエフェクトを返す（画面がレイヤーに足す）。
#[tauri::command]
pub fn effect_preset_load() -> Result<Option<Vec<EffectInstance>>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("エフェクトプリセット", &["json"]).pick_file() else { return Ok(None) };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("プリセットを読み込めません: {e}"))?;
    let preset: EffectPreset = serde_json::from_str(&text).map_err(|e| format!("プリセットを読み込めません: {e}"))?;
    Ok(Some(preset.effects))
}

// ---------- 素材・フォント・エクスプレッション ----------

/// 1/2 解像度の代替ファイルを作り、プレビューを軽くする。
#[tauri::command]
pub fn make_proxy(state: State<'_, Shared>, footage: String) -> Result<(String, Snapshot), String> {
    let mut s = lock(&state);
    let item = s.project.footage_item(&FootageId::new(footage.as_str())).cloned().ok_or("素材がありません")?;
    let (p, w, h) = znd_media::make_proxy(&crate::base_dir(s.path.as_deref()), &item, 2).map_err(|e| format!("プロキシを生成できません: {e}"))?;
    if let Some(engine) = &s.engine {
        engine.footage.set_proxy(&item.id, p, w, h);
    }
    s.refresh();
    Ok((format!("プロキシを生成しました（{w}x{h}）"), s.snapshot()))
}

#[tauri::command]
pub fn font_available(family: String) -> bool {
    znd_text::font_available(&family)
}

#[derive(Serialize)]
pub struct ExprErr {
    layer: String,
    path: String,
    line: Option<u32>,
    message: String,
}

/// コンポのエクスプレッションのエラー（その時刻で評価して調べる）。
#[tauri::command]
pub fn expr_errors(state: State<'_, Shared>, comp: String, frame: i64) -> Vec<ExprErr> {
    let s = lock(&state);
    let id = CompId::new(comp);
    let Some(t) = s.project.comp(&id).map(|c| c.frame_rate.frame_to_time(frame)) else { return Vec::new() };
    let project = s.project.clone();
    drop(s);
    znd_script::ExprEngine::new()
        .check(&project, &id, t)
        .into_iter()
        .map(|e| ExprErr { layer: e.layer, path: e.path, line: e.line, message: e.message })
        .collect()
}

/// ファイルを選ぶ（エフェクトのファイル名の欄など）。
#[tauri::command]
pub fn pick_file() -> Option<String> {
    rfd::FileDialog::new().pick_file().map(|p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_items_go_before_paint_and_animators_can_be_built() {
        let l = Layer::new("s", "s", LayerKind::Shape { items: vec![ShapeItem::rect(1.0, 1.0), ShapeItem::fill([1.0; 4])] }, Transform::centered(0.0, 0.0, 10.0, 10.0), Time::from_seconds(1.0));
        let l = content_op(l, ContentOp::AddShapeItem { key: "trim".into() }).unwrap();
        let LayerKind::Shape { items } = &l.kind else { panic!() };
        assert!(matches!(items[1], ShapeItem::Trim { .. }));
        assert!(matches!(items[2], ShapeItem::Fill { .. }));
        for (key, _) in SHAPE_ITEMS {
            assert!(shape_item(key).is_some(), "{key}");
        }

        let mut doc = text::TextDocument::new("abc", 10.0);
        doc.justify = text::Justify::Center;
        let t = Layer::new("t", "t", LayerKind::Text { document: doc, animators: Vec::new(), path: None, per_char_3d: false, text_expression: None }, Transform::centered(0.0, 0.0, 10.0, 10.0), Time::from_seconds(1.0));
        let t = content_op(t, ContentOp::AddAnimator).unwrap();
        let t = content_op(t, ContentOp::AnimatorProp { index: 0, key: "opacity".into(), on: true }).unwrap();
        let info = animators(t.clone());
        assert_eq!(info[0].props, vec!["opacity"]);
        let t = content_op(t, ContentOp::AnimatorUnit { index: 0, unit: SelectorUnit::Words }).unwrap();
        assert_eq!(animators(t).first().unwrap().unit, Some(SelectorUnit::Words));
    }
}
