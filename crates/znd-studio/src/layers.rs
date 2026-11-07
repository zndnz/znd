//! レイヤーの雛形づくりと、トランスフォームの評価。

use serde::Serialize;
use znd_core::text::Justify;
use znd_core::*;

/// 新しいレイヤーを作る。`kind` は画面の作成メニューの名前。再生ヘッドの位置から始める。
pub fn make(project: &Project, comp_id: &CompId, kind: &str, frame: i64) -> Result<Layer, String> {
    let comp = project.comp(comp_id).ok_or("コンポがありません")?;
    let (w, h, dur) = (comp.width, comp.height, comp.duration);
    let shape = |items: Vec<ShapeItem>| LayerKind::Shape { items };
    let light = |light: LightType| LayerKind::Light {
        light,
        color: [1.0, 1.0, 1.0],
        intensity: Property::constant(100.0),
        cone_angle: Property::constant(90.0),
        cone_feather: Property::constant(50.0),
        casts_shadows: false,
        shadow_darkness: 100.0,
        shadow_diffusion: 0.0,
    };
    let (name, layer_kind) = match kind {
        "text" => {
            let mut doc = TextDocument::new("テキスト", 96.0);
            doc.justify = Justify::Center;
            ("テキスト", LayerKind::Text { document: doc, animators: Vec::new(), path: None, per_char_3d: false, text_expression: None })
        }
        "rect" => ("矩形", shape(vec![ShapeItem::rect(300.0, 200.0), ShapeItem::fill([1.0, 1.0, 1.0, 1.0])])),
        "ellipse" => ("楕円", shape(vec![ShapeItem::ellipse(300.0, 300.0), ShapeItem::fill([1.0, 1.0, 1.0, 1.0])])),
        "star" => ("スター", shape(vec![ShapeItem::star(5.0, 200.0, 90.0), ShapeItem::fill([1.0, 0.85, 0.2, 1.0])])),
        "solid" => ("平面", LayerKind::Solid { width: w, height: h, color: [0.5, 0.5, 0.5] }),
        "null" => ("ヌル", LayerKind::Null),
        "adjustment" => ("調整レイヤー", LayerKind::Adjustment),
        "light_point" => ("ライト（ポイント）", light(LightType::Point)),
        "light_spot" => ("ライト（スポット）", light(LightType::Spot)),
        "light_parallel" => ("ライト（平行）", light(LightType::Parallel)),
        "light_ambient" => ("ライト（アンビエント）", light(LightType::Ambient)),
        "camera" => {
            let id = project.fresh_id("camera");
            let mut cam = camera_import::default_camera(&id, w, h, dur);
            place_at(&mut cam, comp, frame, project);
            return Ok(cam);
        }
        other if other.starts_with("precomp:") => {
            let id = CompId::new(&other["precomp:".len()..]);
            let nested = project.comp(&id).ok_or("コンポがありません")?;
            if &id == comp_id {
                return Err("コンポを自分自身の中には置けません".into());
            }
            (nested.name.as_str(), LayerKind::Precomp { comp: id })
        }
        other => match other.strip_prefix("footage:").and_then(|id| project.footage_item(&FootageId::new(id))) {
            // 素材のレイヤー（音だけの素材は音声のレイヤー）
            Some(item) => {
                let id = item.id.clone();
                let kind = if item.width == 0 { LayerKind::Audio { footage: id } } else { LayerKind::Footage { footage: id } };
                (item.name.as_str(), kind)
            }
            None => return Err(format!("作れないレイヤーの種類です: {other}")),
        },
    };
    let (sw, sh) = project.source_size(&layer_kind).unwrap_or((0.0, 0.0));
    let mut layer = Layer::new(project.fresh_id("layer"), name, layer_kind, Transform::centered(sw, sh, w as f64, h as f64), dur);
    if matches!(layer.kind, LayerKind::Light { .. }) {
        layer.transform.position_z = Property::constant(-500.0);
    }
    place_at(&mut layer, comp, frame, project);
    Ok(layer)
}

/// 再生ヘッド（最後のフレームより後ろにはしない）から始め、素材は素材の長さだけ、それ以外はコンポの終わりまで置く。
fn place_at(layer: &mut Layer, comp: &Composition, frame: i64, project: &Project) {
    let last = (comp.frame_count() - 1).max(0);
    let t = comp.frame_rate.frame_to_time(frame.clamp(0, last));
    let len = match &layer.kind {
        LayerKind::Footage { footage } | LayerKind::Audio { footage } => {
            project.footage_item(footage).and_then(|f| f.duration.or(f.audio.map(|a| a.duration)))
        }
        _ => None,
    };
    layer.in_point = t;
    layer.start_time = t;
    layer.out_point = len.map(|d| (t + d).min(comp.duration)).unwrap_or(comp.duration);
}

/// トランスフォームの、ある時刻の値（キーフレームを補間したもの。エクスプレッションは含まない）。
#[derive(Serialize)]
pub struct Evaluated {
    pub anchor: [f64; 2],
    pub position: [f64; 2],
    pub scale: [f64; 2],
    pub rotation: f64,
    pub opacity: f64,
    pub position_z: f64,
    pub anchor_z: f64,
    pub rotation_x: f64,
    pub rotation_y: f64,
}

pub fn evaluate(layer: &Layer, t: Time) -> Evaluated {
    let tr = &layer.transform;
    Evaluated {
        anchor: tr.anchor.eval(t),
        position: tr.position.eval(t),
        scale: tr.scale.eval(t),
        rotation: tr.rotation.eval(t),
        opacity: tr.opacity.eval(t),
        position_z: tr.position_z.eval(t),
        anchor_z: tr.anchor_z.eval(t),
        rotation_x: tr.rotation_x.eval(t),
        rotation_y: tr.rotation_y.eval(t),
    }
}
