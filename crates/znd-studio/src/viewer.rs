//! ビューア上の直接操作: レイヤーの選択と移動、モーションパスの編集、マスクパスの作成（ペン・矩形・楕円）と編集。
//! 補助線（レイヤーの枠・アンカーポイント・モーションパス・マスク・描きかけのペン）はコンポ座標で画面に渡し、
//! 画面が SVG で描く。座標の変換と当たり判定・編集は `znd_core::edit` が決める。

use crate::{lock, Shared, Snapshot, Studio};
use serde::{Deserialize, Serialize};
use tauri::State;
use znd_core::edit::{self, MotionHit, PathHit, PenPath};
use znd_core::eval::world_matrix;
use znd_core::shape::{BezPath, Mask};
use znd_core::*;

fn apply(m: &Affine, p: [f64; 2]) -> [f64; 2] {
    let (x, y) = m.apply(p[0], p[1]);
    [x, y]
}

fn matrix_scale(m: &Affine) -> f64 {
    (m.a * m.d - m.b * m.c).abs().sqrt().max(1e-9)
}

/// ベジェパスを折れ線にする（レイヤー座標）。
fn flatten(path: &BezPath) -> Vec<[f64; 2]> {
    const STEPS: usize = 16;
    let mut out = Vec::new();
    for seg in 0..path.segment_count() {
        let c = path.segment(seg);
        for i in 0..STEPS {
            out.push(edit::bezier_point(&c, i as f64 / STEPS as f64));
        }
    }
    if let Some(last) = path.vertices.last() {
        out.push(if path.closed { path.vertices[0].p } else { last.p });
    }
    out
}

/// 対角の 2 点から矩形・楕円のパスを作る。`square` なら縦横を同じ長さにする。小さすぎれば `None`。
fn shape_path(ellipse: bool, a: [f64; 2], b: [f64; 2], square: bool) -> Option<BezPath> {
    let (mut w, mut h) = (b[0] - a[0], b[1] - a[1]);
    if square {
        let s = w.abs().max(h.abs());
        (w, h) = (s * w.signum(), s * h.signum());
    }
    if w.abs() < 1.0 || h.abs() < 1.0 {
        return None;
    }
    let (x, y) = (a[0].min(a[0] + w), a[1].min(a[1] + h));
    Some(if ellipse { BezPath::ellipse([x + w.abs() / 2.0, y + h.abs() / 2.0], [w.abs() / 2.0, h.abs() / 2.0]) } else { BezPath::rect(x, y, w.abs(), h.abs()) })
}

/// 掴んだもの（画面とやり取りする形）。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Drag {
    /// レイヤーの移動。`last` は直前のポインタ位置（コンポ座標）。
    Layer { last: [f64; 2] },
    MotionKey { index: usize },
    MotionTanIn { index: usize },
    MotionTanOut { index: usize },
    MaskVertex { mask: usize, index: usize },
    MaskTanIn { mask: usize, index: usize },
    MaskTanOut { mask: usize, index: usize },
    /// 矩形・楕円マスクの対角の始点（レイヤー座標）。
    NewShape { start: [f64; 2] },
    /// ペンで足したばかりの頂点の接線を引っ張っている。
    PenTangent,
}

impl Drag {
    fn motion(h: MotionHit) -> Drag {
        match h {
            MotionHit::Key(index) => Drag::MotionKey { index },
            MotionHit::TanIn(index) => Drag::MotionTanIn { index },
            MotionHit::TanOut(index) => Drag::MotionTanOut { index },
        }
    }

    fn mask(mask: usize, h: PathHit) -> Option<Drag> {
        match h {
            PathHit::Vertex(index) => Some(Drag::MaskVertex { mask, index }),
            PathHit::TanIn(index) => Some(Drag::MaskTanIn { mask, index }),
            PathHit::TanOut(index) => Some(Drag::MaskTanOut { mask, index }),
            PathHit::Segment(..) => None,
        }
    }
}

/// ビューアの状態（画面から毎回送る）。
#[derive(Deserialize)]
pub struct At {
    comp: String,
    frame: i64,
    layer: Option<String>,
    /// 編集中のマスク。
    mask: usize,
    /// 画面上で掴める距離（コンポ座標）。
    grab: f64,
}

struct Geo<'a> {
    comp: &'a Composition,
    layer: Option<&'a Layer>,
    t: Time,
    world: Option<Affine>,
    inv: Option<Affine>,
    parent: Option<Affine>,
    parent_inv: Option<Affine>,
    grab_layer: f64,
    grab_parent: f64,
}

fn geo<'a>(project: &'a Project, at: &At) -> Option<Geo<'a>> {
    let comp = project.comp(&CompId::new(at.comp.as_str()))?;
    let t = comp.frame_rate.frame_to_time(at.frame);
    let layer = at.layer.as_ref().and_then(|id| comp.layer(&LayerId::new(id.as_str())));
    let world = layer.map(|l| world_matrix(comp, l, t));
    let parent = layer.map(|l| edit::parent_matrix(comp, l, t));
    Some(Geo {
        comp,
        layer,
        t,
        inv: world.and_then(|m| m.inverse()),
        parent_inv: parent.and_then(|m| m.inverse()),
        grab_layer: world.map(|m| at.grab / matrix_scale(&m)).unwrap_or(0.0),
        grab_parent: parent.map(|m| at.grab / matrix_scale(&m)).unwrap_or(0.0),
        world,
        parent,
    })
}

// ---------- 補助線 ----------

#[derive(Serialize)]
pub struct VertexInfo {
    p: [f64; 2],
    /// 接線の先端（無ければ `None`）。
    tan_in: Option<[f64; 2]>,
    tan_out: Option<[f64; 2]>,
}

#[derive(Serialize)]
pub struct MaskInfo {
    line: Vec<[f64; 2]>,
    vertices: Vec<VertexInfo>,
}

#[derive(Serialize)]
pub struct MotionKeyInfo {
    p: [f64; 2],
    handles: Option<([f64; 2], [f64; 2])>,
}

/// 選んでいるレイヤーの補助線（すべてコンポ座標）。
#[derive(Serialize, Default)]
pub struct Overlay {
    frame: Option<[[f64; 2]; 4]>,
    anchor: Option<[f64; 2]>,
    /// アンカーポイントのまわりの回転（度）。回転ツールが使う。
    rotation: f64,
    motion: Vec<[f64; 2]>,
    motion_keys: Vec<MotionKeyInfo>,
    /// 再生ヘッドの位置にあるモーションパスのキー。
    motion_now: Option<usize>,
    masks: Vec<MaskInfo>,
    /// 描きかけのペンのパス。
    pen: Vec<VertexInfo>,
}

fn vertex_info(m: &Affine, v: &PathVertex) -> VertexInfo {
    let tip = |t: [f64; 2]| (t != [0.0; 2]).then(|| apply(m, [v.p[0] + t[0], v.p[1] + t[1]]));
    VertexInfo { p: apply(m, v.p), tan_in: tip(v.tan_in), tan_out: tip(v.tan_out) }
}

#[tauri::command]
pub fn viewer_overlay(state: State<'_, Shared>, at: At) -> Overlay {
    let s = lock(&state);
    overlay(&s, &at)
}

fn overlay(s: &Studio, at: &At) -> Overlay {
    let Some(g) = geo(&s.project, at) else { return Overlay::default() };
    let (Some(layer), Some(world), Some(parent)) = (g.layer, g.world, g.parent) else { return Overlay::default() };
    let t = g.t;
    let pos = &layer.transform.position;
    let mut out = Overlay {
        frame: s.project.source_size(&layer.kind).map(|(w, h)| [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|p| apply(&world, p))),
        anchor: Some(apply(&world, layer.transform.anchor.eval(t))),
        rotation: layer.transform.rotation.eval(t),
        ..Default::default()
    };
    if pos.keyframes.len() >= 2 {
        out.motion = edit::motion_path_points(pos, 24).into_iter().map(|p| apply(&parent, p)).collect();
        out.motion_now = pos.key_index_at(t);
        out.motion_keys = edit::motion_handles(pos)
            .into_iter()
            .map(|(k, h)| MotionKeyInfo { p: apply(&parent, k), handles: h.map(|(a, b)| (apply(&parent, a), apply(&parent, b))) })
            .collect();
    }
    out.masks = layer
        .masks
        .iter()
        .map(|mask| {
            let path = mask.path.eval(t);
            MaskInfo { line: flatten(&path).into_iter().map(|p| apply(&world, p)).collect(), vertices: path.vertices.iter().map(|v| vertex_info(&world, v)).collect() }
        })
        .collect();
    if let Some((id, pen)) = &s.pen {
        if id == &layer.id {
            out.pen = pen.vertices.iter().map(|v| vertex_info(&world, v)).collect();
        }
    }
    out
}

// ---------- 操作 ----------

#[derive(Serialize, Default)]
pub struct Outcome {
    snapshot: Option<Snapshot>,
    /// 掴んだもの（ドラッグを続けるときに送り返す）。
    drag: Option<Drag>,
    /// 選ぶレイヤー（`Some(None)` で選択を外す。`None` なら変えない）。
    #[serde(skip_serializing_if = "Option::is_none")]
    select: Option<Option<String>>,
    /// 編集中のマスクと、選んだ頂点。
    mask: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vertex: Option<Option<usize>>,
    status: Option<String>,
}

impl Studio {
    /// レイヤーを書き換える（ロック中のレイヤー・ほかの人がロックしたコンポは書き換えない）。
    fn edit_layer_at(&mut self, at: &At, label: &str, coalesce: bool, gesture: Option<u64>, f: impl FnOnce(&mut Layer, &Composition)) -> Result<(), String> {
        let comp_id = CompId::new(at.comp.as_str());
        if let Some(owner) = self.locked_by_other(&comp_id) {
            return Err(format!("このコンポは {owner} がロックしているため編集できません"));
        }
        let id = LayerId::new(at.layer.clone().ok_or("レイヤーを選択してください")?);
        let mut p = (*self.project).clone();
        let c = p.comp_mut(&comp_id).ok_or("コンポがありません")?;
        let snapshot = c.clone();
        let l = c.layer_mut(&id).ok_or("レイヤーがありません")?;
        if l.locked {
            return Err(format!("「{}」はロックされています", l.name));
        }
        f(l, &snapshot);
        self.apply_in(p, label, coalesce, gesture);
        Ok(())
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Select,
    Pen,
    Rect,
    Ellipse,
}

/// ポインタを押した: 掴むものを決める（ペンは頂点を足す）。
#[tauri::command]
pub fn viewer_down(state: State<'_, Shared>, at: At, tool: Tool, point: [f64; 2]) -> Outcome {
    let mut s = lock(&state);
    down(&mut s, &at, tool, point)
}

fn down(s: &mut Studio, at: &At, tool: Tool, c0: [f64; 2]) -> Outcome {
    let project = s.project.clone();
    let Some(g) = geo(&project, at) else { return Outcome::default() };
    let mut out = Outcome::default();
    match tool {
        Tool::Select => {
            if let (Some(l), Some(inv)) = (g.layer, g.inv) {
                let p0 = apply(&inv, c0);
                // 1. マスクの頂点・接線（編集中のマスクを優先し、無ければほかのマスクも探す）
                for mi in std::iter::once(at.mask).chain(0..l.masks.len()) {
                    let Some(mask) = l.masks.get(mi) else { continue };
                    if let Some(d) = edit::path_hit(&mask.path.eval(g.t), p0, g.grab_layer).and_then(|h| Drag::mask(mi, h)) {
                        out.mask = Some(mi);
                        if let Drag::MaskVertex { index, .. } | Drag::MaskTanIn { index, .. } | Drag::MaskTanOut { index, .. } = d {
                            out.vertex = Some(Some(index));
                        }
                        out.drag = Some(d);
                        return out;
                    }
                }
                // 2. モーションパスのキーと接線
                if let Some(pinv) = g.parent_inv {
                    if let Some(hit) = edit::motion_hit(&l.transform.position, apply(&pinv, c0), g.grab_parent) {
                        out.drag = Some(Drag::motion(hit));
                        return out;
                    }
                }
            }
            // 3. その位置のレイヤー。何も無ければ、選択中の（寸法を持たない）レイヤーを動かす
            match edit::layer_at(&project, g.comp, g.t, c0) {
                Some(id) => {
                    out.select = Some(Some(id.0));
                    out.drag = Some(Drag::Layer { last: c0 });
                }
                None if g.layer.is_some_and(|l| !l.locked && !edit::has_bounds(&project, &l.kind)) => out.drag = Some(Drag::Layer { last: c0 }),
                None => {}
            }
        }
        Tool::Pen => {
            let (Some(l), Some(inv)) = (g.layer, g.inv) else {
                out.status = Some("マスクを描くレイヤーを先に選択してください".into());
                return out;
            };
            let p = apply(&inv, c0);
            let drawing = s.pen.as_ref().is_some_and(|(id, _)| id == &l.id);
            // 描き始める前に既存のパスの上をクリックしたら、そこに頂点を足す
            if !drawing {
                if let Some(PathHit::Segment(seg, u)) = l.masks.get(at.mask).and_then(|m| edit::path_hit(&m.path.eval(g.t), p, g.grab_layer)) {
                    let mi = at.mask;
                    match s.edit_layer_at(at, "頂点を追加", false, None, |l, _| {
                        if let Some(mask) = l.masks.get_mut(mi) {
                            edit::mask_insert_vertex(mask, seg, u);
                        }
                    }) {
                        Ok(()) => {
                            out.snapshot = Some(s.snapshot());
                            out.vertex = Some(Some(seg + 1));
                        }
                        Err(e) => out.status = Some(e),
                    }
                    return out;
                }
            }
            if drawing && s.pen.as_ref().is_some_and(|(_, pen)| pen.closes_at(p, g.grab_layer)) {
                return finish_pen(s, at, true);
            }
            let id = l.id.clone();
            match &mut s.pen {
                Some((pid, pen)) if *pid == id => pen.add(p, None),
                _ => {
                    let mut pen = PenPath::default();
                    pen.add(p, None);
                    s.pen = Some((id, pen));
                }
            }
            out.drag = Some(Drag::PenTangent);
        }
        Tool::Rect | Tool::Ellipse => match g.inv {
            Some(inv) => out.drag = Some(Drag::NewShape { start: apply(&inv, c0) }),
            None => out.status = Some("マスクを描くレイヤーを先に選択してください".into()),
        },
    }
    out
}

/// ドラッグ中: 掴んだものを動かす。`gesture` が同じ間は 1 回の取り消しにまとめる。
#[tauri::command]
pub fn viewer_move(state: State<'_, Shared>, at: At, drag: Drag, point: [f64; 2], alt: bool, gesture: u64) -> Outcome {
    let mut s = lock(&state);
    let project = s.project.clone();
    let Some(g) = geo(&project, &at) else { return Outcome::default() };
    let t = g.t;
    let before = s.revision;
    let mut out = Outcome { drag: Some(drag), ..Default::default() };
    let g_ = Some(gesture);
    let r = match drag {
        Drag::Layer { last } => {
            let delta = [point[0] - last[0], point[1] - last[1]];
            out.drag = Some(Drag::Layer { last: point });
            let comp_id = CompId::new(at.comp.as_str());
            let id = at.layer.clone().map(LayerId::new);
            match (id, s.locked_by_other(&comp_id)) {
                (Some(id), None) => {
                    let mut p = (*s.project).clone();
                    if let Some(c) = p.comp_mut(&comp_id) {
                        if c.layer(&id).is_some_and(|l| !l.locked) {
                            edit::move_layer(c, &id, t, delta);
                            s.apply_in(p, "レイヤーを移動", true, g_);
                        }
                    }
                    Ok(())
                }
                (_, Some(owner)) => Err(format!("このコンポは {owner} がロックしているため編集できません")),
                _ => Ok(()),
            }
        }
        Drag::MotionKey { index } | Drag::MotionTanIn { index } | Drag::MotionTanOut { index } => match g.parent_inv {
            Some(pinv) => {
                let hit = match drag {
                    Drag::MotionKey { .. } => MotionHit::Key(index),
                    Drag::MotionTanIn { .. } => MotionHit::TanIn(index),
                    _ => MotionHit::TanOut(index),
                };
                let to = apply(&pinv, point);
                s.edit_layer_at(&at, "モーションパスを編集", true, g_, |l, _| edit::motion_drag(&mut l.transform.position, hit, to, !alt))
            }
            None => Ok(()),
        },
        Drag::MaskVertex { mask, index } | Drag::MaskTanIn { mask, index } | Drag::MaskTanOut { mask, index } => match g.inv {
            Some(inv) => {
                let hit = match drag {
                    Drag::MaskVertex { .. } => PathHit::Vertex(index),
                    Drag::MaskTanIn { .. } => PathHit::TanIn(index),
                    _ => PathHit::TanOut(index),
                };
                let to = apply(&inv, point);
                s.edit_layer_at(&at, "マスクパスを編集", true, g_, |l, _| {
                    if let Some(m) = l.masks.get_mut(mask) {
                        edit::edit_mask_path(m, t, |p| edit::path_drag(p, hit, to, !alt));
                    }
                })
            }
            None => Ok(()),
        },
        Drag::PenTangent => {
            if let (Some(inv), Some((_, pen))) = (g.inv, s.pen.as_mut()) {
                let to = apply(&inv, point);
                if let Some(v) = pen.vertices.last_mut() {
                    v.tan_out = [to[0] - v.p[0], to[1] - v.p[1]];
                    v.tan_in = [-v.tan_out[0], -v.tan_out[1]];
                }
            }
            Ok(())
        }
        Drag::NewShape { .. } => Ok(()),
    };
    match r {
        Ok(()) if s.revision != before => out.snapshot = Some(s.snapshot()),
        Ok(()) => {}
        Err(e) => out.status = Some(e),
    }
    out
}

/// ポインタを離した: 矩形・楕円のマスクを作る。
#[tauri::command]
pub fn viewer_up(state: State<'_, Shared>, at: At, tool: Tool, drag: Drag, point: [f64; 2], shift: bool) -> Outcome {
    let mut s = lock(&state);
    let Drag::NewShape { start } = drag else { return Outcome::default() };
    let project = s.project.clone();
    let Some(g) = geo(&project, &at) else { return Outcome::default() };
    let (Some(inv), Some(l)) = (g.inv, g.layer) else { return Outcome::default() };
    let Some(path) = shape_path(tool == Tool::Ellipse, start, apply(&inv, point), shift) else { return Outcome::default() };
    let n = l.masks.len();
    match s.edit_layer_at(&at, "マスクを追加", false, None, |l, _| l.masks.push(Mask::new(path))) {
        Ok(()) => Outcome { snapshot: Some(s.snapshot()), mask: Some(n), vertex: Some(None), status: Some(format!("マスク {} を追加しました", n + 1)), ..Default::default() },
        Err(e) => Outcome { status: Some(e), ..Default::default() },
    }
}

/// クリック（動かさずに離した）。マスクの頂点を選ぶ（Option で角 ⇄ 滑らか）、モーションパスのキーをダブルクリックで
/// 接線ハンドルを出す、何も無い所でレイヤーを選ぶ・選択を外す。
#[tauri::command]
pub fn viewer_click(state: State<'_, Shared>, at: At, point: [f64; 2], alt: bool, double: bool) -> Outcome {
    let mut s = lock(&state);
    let project = s.project.clone();
    let Some(g) = geo(&project, &at) else { return Outcome::default() };
    let mut out = Outcome::default();
    if let (Some(l), Some(inv)) = (g.layer, g.inv) {
        let p = apply(&inv, point);
        for mi in std::iter::once(at.mask).chain(0..l.masks.len()) {
            let Some(mask) = l.masks.get(mi) else { continue };
            if let Some(PathHit::Vertex(i)) = edit::path_hit(&mask.path.eval(g.t), p, g.grab_layer) {
                out.mask = Some(mi);
                out.vertex = Some(Some(i));
                if alt {
                    let t = g.t;
                    match s.edit_layer_at(&at, "頂点の種類", false, None, |l, _| {
                        if let Some(m) = l.masks.get_mut(mi) {
                            edit::edit_mask_path(m, t, |p| edit::toggle_smooth(p, i));
                        }
                    }) {
                        Ok(()) => out.snapshot = Some(s.snapshot()),
                        Err(e) => out.status = Some(e),
                    }
                }
                return out;
            }
        }
        if double {
            if let Some(MotionHit::Key(i)) = g.parent_inv.and_then(|pinv| edit::motion_hit(&l.transform.position, apply(&pinv, point), g.grab_parent)) {
                match s.edit_layer_at(&at, "接線ハンドル", false, None, |l, _| edit::motion_make_manual(&mut l.transform.position, i)) {
                    Ok(()) => out.snapshot = Some(s.snapshot()),
                    Err(e) => out.status = Some(e),
                }
                return out;
            }
        }
    }
    // 何も無い所をクリックしたら選択を外す（寸法を持たないレイヤーは外さない: 画面上で選び直せないため）
    match edit::layer_at(&project, g.comp, g.t, point) {
        Some(id) => out.select = Some(Some(id.0)),
        None if g.layer.is_some_and(|l| edit::has_bounds(&project, &l.kind)) => out.select = Some(None),
        None => {}
    }
    out.vertex = Some(None);
    out
}

fn finish_pen(s: &mut Studio, at: &At, closed: bool) -> Outcome {
    let Some((id, pen)) = s.pen.take() else { return Outcome::default() };
    let Some(mask) = pen.finish(closed) else { return Outcome { status: Some("マスクにするには頂点が足りません".into()), ..Default::default() } };
    let at = At { comp: at.comp.clone(), frame: at.frame, layer: Some(id.0.clone()), mask: 0, grab: 0.0 };
    let n = s.project.comp(&CompId::new(at.comp.as_str())).and_then(|c| c.layer(&id)).map(|l| l.masks.len()).unwrap_or(0);
    match s.edit_layer_at(&at, "マスクを追加", false, None, |l, _| l.masks.push(mask)) {
        Ok(()) => Outcome { snapshot: Some(s.snapshot()), mask: Some(n), vertex: Some(None), status: Some(format!("マスク {} を追加しました", n + 1)), ..Default::default() },
        Err(e) => Outcome { status: Some(e), ..Default::default() },
    }
}

/// 描きかけのペンのパスを確定する（Enter で開いたまま）か、捨てる（Esc）。
#[tauri::command]
pub fn pen_finish(state: State<'_, Shared>, at: At, closed: bool, cancel: bool) -> Outcome {
    let mut s = lock(&state);
    if cancel {
        s.pen = None;
        return Outcome::default();
    }
    finish_pen(&mut s, &at, closed)
}

/// 描きかけのペンがあるか。
#[tauri::command]
pub fn pen_active(state: State<'_, Shared>) -> bool {
    lock(&state).pen.is_some()
}

/// 選んだマスクの頂点を消す。
#[tauri::command]
pub fn mask_remove_vertex(state: State<'_, Shared>, at: At, vertex: usize) -> Outcome {
    let mut s = lock(&state);
    let mut removed = false;
    let mi = at.mask;
    match s.edit_layer_at(&at, "頂点を削除", false, None, |l, _| {
        if let Some(m) = l.masks.get_mut(mi) {
            removed = edit::mask_remove_vertex(m, vertex);
        }
    }) {
        Ok(()) if removed => Outcome { snapshot: Some(s.snapshot()), vertex: Some(None), ..Default::default() },
        Ok(()) => Outcome { status: Some("これ以上は頂点を減らせません".into()), ..Default::default() },
        Err(e) => Outcome { status: Some(e), ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio() -> Studio {
        let mut p = Project::new("t");
        let mut c = Composition::new("c", "c", 200, 200, FrameRate::FPS_30, Time::from_seconds(2.0));
        c.layers.push(Layer::new("l", "平面", LayerKind::Solid { width: 100, height: 100, color: [1.0; 3] }, Transform::centered(100.0, 100.0, 200.0, 200.0), c.duration));
        p.compositions.push(c);
        Studio::new(p, None)
    }

    fn at() -> At {
        At { comp: "c".into(), frame: 0, layer: Some("l".into()), mask: 0, grab: 4.0 }
    }

    #[test]
    fn clicking_selects_the_layer_under_the_pointer() {
        let mut s = studio();
        let o = down(&mut s, &At { layer: None, ..at() }, Tool::Select, [100.0, 100.0]);
        assert_eq!(o.select, Some(Some("l".into())));
        assert_eq!(o.drag, Some(Drag::Layer { last: [100.0, 100.0] }));
        let o = down(&mut s, &At { layer: None, ..at() }, Tool::Select, [5.0, 5.0]);
        assert!(o.drag.is_none());
    }

    #[test]
    fn rect_masks_are_made_in_layer_coordinates() {
        let s = studio();
        let o = overlay(&s, &at());
        assert_eq!(o.frame.unwrap()[0], [50.0, 50.0]);
        assert_eq!(o.anchor, Some([100.0, 100.0]));
        assert!(shape_path(false, [0.0, 0.0], [0.5, 10.0], false).is_none());
        let p = shape_path(true, [0.0, 0.0], [10.0, 4.0], true).unwrap();
        assert_eq!(p.vertices.len(), 4);
    }

    #[test]
    fn the_pen_adds_vertices_and_closes_into_a_mask() {
        let mut s = studio();
        for p in [[60.0, 60.0], [140.0, 60.0], [140.0, 140.0]] {
            assert_eq!(down(&mut s, &at(), Tool::Pen, p).drag, Some(Drag::PenTangent));
        }
        assert_eq!(overlay(&s, &at()).pen.len(), 3);
        // 始点をクリックすると閉じてマスクになる
        let o = down(&mut s, &at(), Tool::Pen, [60.0, 60.0]);
        assert!(o.snapshot.is_some(), "{:?}", o.status);
        assert!(s.pen.is_none());
        let l = s.project.comp(&CompId::new("c")).unwrap().layer(&LayerId::new("l")).unwrap();
        assert_eq!(l.masks.len(), 1);
        assert!(l.masks[0].path.value.closed);
        // レイヤー座標（左上が 0）になっている
        assert_eq!(l.masks[0].path.value.vertices[0].p, [10.0, 10.0]);
    }
}
