//! ビューア上の直接操作: レイヤーの選択と移動、モーションパスの編集、マスクパスの作成と編集。
//!
//! ここはポインタの位置を座標に直して渡すだけで、何がどう変わるかは `znd_core::edit` が決める。

use crate::App;
use eframe::egui::{self, pos2, Color32, Pos2, Rect, Stroke};
use znd_core::edit::{self, MotionHit, PathHit};
use znd_core::eval::world_matrix;
use znd_core::shape::{BezPath, Mask};
use znd_core::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    /// 選択・移動、モーションパスとマスクの頂点の編集。
    Select,
    /// マスクパスを描く。既存のパスの上では頂点を足す。
    Pen,
    RectMask,
    EllipseMask,
}

impl Tool {
    pub const ALL: [(Tool, &'static str, &'static str); 4] = [
        (Tool::Select, "選択", "選択・移動。モーションパスとマスクの頂点を掴んで動かせます (V)"),
        (Tool::Pen, "ペン", "クリックで頂点、ドラッグで曲線。始点をクリックで閉じる。Enter で開いたまま確定、Esc で取り消し (G)"),
        (Tool::RectMask, "矩形マスク", "ドラッグで矩形のマスクを作ります (R)"),
        (Tool::EllipseMask, "楕円マスク", "ドラッグで楕円のマスクを作ります (O)"),
    ];
}

/// ドラッグ中の操作。
pub(crate) enum ViewerDrag {
    /// レイヤーの移動。`last` は直前のポインタ位置（コンポ座標）。
    Layer {
        last: [f64; 2],
    },
    Motion(MotionHit),
    Mask(PathHit),
    /// 矩形・楕円マスクの対角の始点（レイヤー座標）。
    NewShape {
        start: [f64; 2],
    },
    /// ペンで足したばかりの頂点の接線を引っ張っている。
    PenTangent,
}

/// 画面上で掴める範囲（px）。
const GRAB: f32 = 8.0;
const MASK_COLOR: Color32 = Color32::from_rgb(255, 210, 60);
const PATH_COLOR: Color32 = Color32::from_rgb(120, 220, 255);

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

impl App {
    /// ツールの切り替えボタン（ビューア上部）。
    pub(crate) fn tool_buttons(&mut self, ui: &mut egui::Ui) {
        for (tool, label, tip) in Tool::ALL {
            if ui.selectable_label(self.tool == tool, label).on_hover_text(tip).clicked() {
                self.set_tool(tool);
            }
        }
    }

    pub(crate) fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.pen = None;
        self.viewer_drag = None;
    }

    /// 描きかけのペンのパスをマスクとして確定する。
    fn finish_pen(&mut self, closed: bool) {
        let (Some(pen), Some(id)) = (self.pen.take(), self.selected.clone()) else { return };
        match pen.finish(closed) {
            Some(mask) => self.push_mask(&id, mask),
            None => self.status = "マスクにするには頂点が足りません".into(),
        }
    }

    fn push_mask(&mut self, id: &LayerId, mask: Mask) {
        let count = self.shared.comp(&self.comp_id).and_then(|c| c.layer(id)).map(|l| l.masks.len()).unwrap_or(0);
        self.edit_layer("マスク追加", false, id, |l| l.masks.push(mask));
        self.active_mask = count;
        self.active_vertex = None;
        self.status = format!("マスク {} を追加しました", count + 1);
    }

    /// ビューアの画像の上での操作と、補助線の描画。
    /// `img` は画像の表示領域、`scale` は画面 px / コンポ px。
    pub(crate) fn viewer_tools(&mut self, ui: &mut egui::Ui, resp: &egui::Response, img: Rect, scale: f32) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let t = comp.frame_rate.frame_to_time(self.frame);
        let to_comp = |p: Pos2| [((p.x - img.min.x) / scale) as f64, ((p.y - img.min.y) / scale) as f64];
        let to_screen = |c: [f64; 2]| pos2(img.min.x + c[0] as f32 * scale, img.min.y + c[1] as f32 * scale);
        let apply = |m: &Affine, p: [f64; 2]| {
            let (x, y) = m.apply(p[0], p[1]);
            [x, y]
        };

        let layer = self.selected.as_ref().and_then(|id| comp.layer(id));
        let world = layer.map(|l| world_matrix(comp, l, t));
        let inv = world.and_then(|m| m.inverse());
        let parent = layer.map(|l| edit::parent_matrix(comp, l, t));
        let parent_inv = parent.and_then(|m| m.inverse());
        // 画面上の GRAB px に相当する、レイヤー座標・親座標での距離
        let grab_layer = world.map(|m| GRAB as f64 / scale as f64 / matrix_scale(&m)).unwrap_or(0.0);
        let grab_parent = parent.map(|m| GRAB as f64 / scale as f64 / matrix_scale(&m)).unwrap_or(0.0);
        if layer.is_some_and(|l| self.active_mask >= l.masks.len()) {
            self.active_mask = 0;
            self.active_vertex = None;
        }

        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let press = ui.input(|i| i.pointer.press_origin());

        // ---------- 操作 ----------
        match self.tool {
            Tool::Select => {
                if resp.drag_started() {
                    if let Some(c0) = press.map(to_comp) {
                        let mut drag = None;
                        if let (Some(l), Some(inv)) = (layer, inv) {
                            let p0 = apply(&inv, c0);
                            // 1. マスクの頂点・接線（編集中のマスクを優先し、無ければ他のマスクも探す）
                            let order = std::iter::once(self.active_mask).chain(0..l.masks.len());
                            for mi in order {
                                let Some(mask) = l.masks.get(mi) else { continue };
                                match edit::path_hit(&mask.path.eval(t), p0, grab_layer) {
                                    Some(hit @ (PathHit::Vertex(i) | PathHit::TanIn(i) | PathHit::TanOut(i))) => {
                                        self.active_mask = mi;
                                        self.active_vertex = Some(i);
                                        drag = Some(ViewerDrag::Mask(hit));
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            // 2. モーションパスのキーと接線
                            if drag.is_none() {
                                if let Some(pinv) = parent_inv {
                                    if let Some(hit) = edit::motion_hit(&l.transform.position, apply(&pinv, c0), grab_parent) {
                                        drag = Some(ViewerDrag::Motion(hit));
                                    }
                                }
                            }
                        }
                        // 3. その位置のレイヤー。何も無ければ、選択中の（寸法を持たない）レイヤーを動かす
                        if drag.is_none() {
                            match edit::layer_at(&shared, comp, t, c0) {
                                Some(id) => {
                                    self.selected = Some(id);
                                    self.selected_footage = None;
                                    drag = Some(ViewerDrag::Layer { last: c0 });
                                }
                                None if layer.is_some_and(|l| !l.locked && !edit::has_bounds(&shared, &l.kind)) => {
                                    drag = Some(ViewerDrag::Layer { last: c0 });
                                }
                                None => {}
                            }
                        }
                        self.viewer_drag = drag;
                    }
                }
                if resp.dragged() {
                    if let (Some(cur), Some(id)) = (pointer.map(to_comp), self.selected.clone()) {
                        let mut drag = self.viewer_drag.take();
                        match &mut drag {
                            Some(ViewerDrag::Layer { last }) => {
                                let delta = [cur[0] - last[0], cur[1] - last[1]];
                                *last = cur;
                                self.edit_comp("レイヤー移動", true, |c| edit::move_layer(c, &id, t, delta));
                            }
                            Some(ViewerDrag::Motion(hit)) => {
                                if let Some(pinv) = parent_inv {
                                    let (hit, to) = (*hit, apply(&pinv, cur));
                                    self.edit_layer("モーションパス編集", true, &id, |l| {
                                        edit::motion_drag(&mut l.transform.position, hit, to, !alt)
                                    });
                                }
                            }
                            Some(ViewerDrag::Mask(hit)) => {
                                if let Some(inv) = inv {
                                    let (hit, to, mi) = (*hit, apply(&inv, cur), self.active_mask);
                                    self.edit_layer("マスクパス編集", true, &id, |l| {
                                        if let Some(mask) = l.masks.get_mut(mi) {
                                            edit::edit_mask_path(mask, t, |p| edit::path_drag(p, hit, to, !alt));
                                        }
                                    });
                                }
                            }
                            _ => {}
                        }
                        self.viewer_drag = drag;
                    }
                }
                if resp.clicked() || resp.double_clicked() {
                    if let Some(c) = pointer.map(to_comp) {
                        let mut handled = false;
                        if let (Some(l), Some(inv), Some(id)) = (layer, inv, self.selected.clone()) {
                            let p = apply(&inv, c);
                            // マスクの頂点: クリックで選択、Option + クリックで角 ⇄ 滑らか
                            for mi in std::iter::once(self.active_mask).chain(0..l.masks.len()) {
                                let Some(mask) = l.masks.get(mi) else { continue };
                                if let Some(PathHit::Vertex(i)) = edit::path_hit(&mask.path.eval(t), p, grab_layer) {
                                    self.active_mask = mi;
                                    self.active_vertex = Some(i);
                                    if alt {
                                        self.edit_layer("頂点の種類", false, &id, |l| {
                                            if let Some(mask) = l.masks.get_mut(mi) {
                                                edit::edit_mask_path(mask, t, |p| edit::toggle_smooth(p, i));
                                            }
                                        });
                                    }
                                    handled = true;
                                    break;
                                }
                            }
                            // モーションパスのキー: ダブルクリックで接線ハンドルを出す
                            if !handled && resp.double_clicked() {
                                if let Some(pinv) = parent_inv {
                                    if let Some(MotionHit::Key(i)) = edit::motion_hit(&l.transform.position, apply(&pinv, c), grab_parent) {
                                        self.edit_layer("接線ハンドル", false, &id, |l| {
                                            edit::motion_make_manual(&mut l.transform.position, i)
                                        });
                                        handled = true;
                                    }
                                }
                            }
                        }
                        if !handled {
                            // 何も無い所をクリックしたら選択を外す（寸法を持たないレイヤーは外さない: 画面上で選び直せないため）
                            match edit::layer_at(&shared, comp, t, c) {
                                Some(id) => self.selected = Some(id),
                                None if layer.is_some_and(|l| edit::has_bounds(&shared, &l.kind)) => self.selected = None,
                                None => {}
                            }
                            self.selected_footage = None;
                            self.active_vertex = None;
                        }
                    }
                }
            }
            Tool::Pen => match (layer, inv) {
                (Some(l), Some(inv)) => {
                    let id = l.id.clone();
                    let add_or_close = |app: &mut App, p: [f64; 2], drag: bool| -> bool {
                        // 描き始める前に既存のパスの上をクリックしたら、そこに頂点を足す
                        if app.pen.is_none() {
                            if let Some(mask) = l.masks.get(app.active_mask) {
                                if let Some(PathHit::Segment(seg, u)) = edit::path_hit(&mask.path.eval(t), p, grab_layer) {
                                    let mi = app.active_mask;
                                    app.edit_layer("頂点を追加", false, &id, |l| {
                                        if let Some(mask) = l.masks.get_mut(mi) {
                                            edit::mask_insert_vertex(mask, seg, u);
                                        }
                                    });
                                    app.active_vertex = Some(seg + 1);
                                    return false;
                                }
                            }
                        }
                        if app.pen.as_ref().is_some_and(|pen| pen.closes_at(p, grab_layer)) {
                            app.finish_pen(true);
                            return false;
                        }
                        app.pen.get_or_insert_with(Default::default).add(p, None);
                        drag
                    };
                    if resp.drag_started() {
                        if let Some(p) = press.map(to_comp).map(|c| apply(&inv, c)) {
                            if add_or_close(self, p, true) {
                                self.viewer_drag = Some(ViewerDrag::PenTangent);
                            }
                        }
                    } else if resp.clicked() {
                        if let Some(p) = pointer.map(to_comp).map(|c| apply(&inv, c)) {
                            add_or_close(self, p, false);
                        }
                    }
                    if resp.dragged() && matches!(self.viewer_drag, Some(ViewerDrag::PenTangent)) {
                        if let (Some(to), Some(v)) =
                            (pointer.map(to_comp).map(|c| apply(&inv, c)), self.pen.as_mut().and_then(|p| p.vertices.last_mut()))
                        {
                            v.tan_out = [to[0] - v.p[0], to[1] - v.p[1]];
                            v.tan_in = [-v.tan_out[0], -v.tan_out[1]];
                        }
                    }
                }
                _ => {
                    if resp.clicked() || resp.drag_started() {
                        self.status = "マスクを描くレイヤーを先に選択してください".into();
                    }
                }
            },
            Tool::RectMask | Tool::EllipseMask => match (layer, inv) {
                (Some(l), Some(inv)) => {
                    if resp.drag_started() {
                        if let Some(p) = press.map(to_comp).map(|c| apply(&inv, c)) {
                            self.viewer_drag = Some(ViewerDrag::NewShape { start: p });
                        }
                    }
                    if resp.drag_stopped() {
                        if let (Some(ViewerDrag::NewShape { start }), Some(end)) =
                            (self.viewer_drag.take(), pointer.map(to_comp).map(|c| apply(&inv, c)))
                        {
                            if let Some(path) = shape_path(self.tool, start, end, shift) {
                                let id = l.id.clone();
                                self.push_mask(&id, Mask::new(path));
                            }
                        }
                    }
                }
                _ => {
                    if resp.clicked() || resp.drag_started() {
                        self.status = "マスクを描くレイヤーを先に選択してください".into();
                    }
                }
            },
        }
        if resp.drag_stopped() {
            self.viewer_drag = None;
            // ドラッグごとに取り消しの単位を分ける
            self.last_edit = None;
        }

        // キー操作（文字入力中は奪わない）
        if !ui.ctx().wants_keyboard_input() {
            let pressed = |k: egui::Key| ui.input(|i| i.key_pressed(k));
            if self.pen.is_some() {
                if pressed(egui::Key::Enter) {
                    self.finish_pen(false);
                } else if pressed(egui::Key::Escape) {
                    self.pen = None;
                }
            } else if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
                if let (Some(i), Some(id)) = (self.active_vertex, self.selected.clone()) {
                    let mi = self.active_mask;
                    let mut removed = false;
                    self.edit_layer("頂点を削除", false, &id, |l| {
                        if let Some(mask) = l.masks.get_mut(mi) {
                            removed = edit::mask_remove_vertex(mask, i);
                        }
                    });
                    if removed {
                        self.active_vertex = None;
                    } else {
                        self.status = "これ以上は頂点を減らせません".into();
                    }
                }
            }
        }

        // ---------- 補助線の描画（編集後の状態で描く） ----------
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let Some(layer) = self.selected.as_ref().and_then(|id| comp.layer(id)) else { return };
        let world = world_matrix(comp, layer, t);
        let parent = edit::parent_matrix(comp, layer, t);
        let painter = ui.painter_at(resp.rect);
        let l2s = |p: [f64; 2]| to_screen(apply(&world, p));
        let p2s = |p: [f64; 2]| to_screen(apply(&parent, p));
        let line = |pts: Vec<Pos2>, stroke: Stroke| {
            painter.add(egui::Shape::line(pts, stroke));
        };

        // レイヤーの枠
        if let Some((w, h)) = shared.source_size(&layer.kind) {
            let corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h], [0.0, 0.0]].map(l2s).to_vec();
            line(corners, Stroke::new(1.0_f32, PATH_COLOR.gamma_multiply(0.6)));
        }
        // アンカーポイント
        let anchor = l2s(layer.transform.anchor.eval(t));
        painter.circle_stroke(anchor, 4.0, Stroke::new(1.0_f32, PATH_COLOR));
        painter.line_segment([anchor - egui::vec2(7.0, 0.0), anchor + egui::vec2(7.0, 0.0)], Stroke::new(1.0_f32, PATH_COLOR));
        painter.line_segment([anchor - egui::vec2(0.0, 7.0), anchor + egui::vec2(0.0, 7.0)], Stroke::new(1.0_f32, PATH_COLOR));

        // モーションパス
        let pos = &layer.transform.position;
        if pos.keyframes.len() >= 2 {
            line(edit::motion_path_points(pos, 24).into_iter().map(p2s).collect(), Stroke::new(1.5_f32, PATH_COLOR));
            let now = pos.key_index_at(t);
            for (i, (key, handles)) in edit::motion_handles(pos).into_iter().enumerate() {
                let k = p2s(key);
                if let Some((tin, tout)) = handles {
                    for h in [tin, tout] {
                        painter.line_segment([k, p2s(h)], Stroke::new(1.0_f32, PATH_COLOR.gamma_multiply(0.7)));
                        painter.circle_filled(p2s(h), 3.0, PATH_COLOR);
                    }
                }
                let fill = if now == Some(i) { Color32::WHITE } else { PATH_COLOR };
                painter.rect_filled(Rect::from_center_size(k, egui::vec2(7.0, 7.0)), 0.0, fill);
                painter.rect_stroke(Rect::from_center_size(k, egui::vec2(7.0, 7.0)), 0.0, Stroke::new(1.0_f32, Color32::BLACK));
            }
        }

        // マスク
        for (mi, mask) in layer.masks.iter().enumerate() {
            let path = mask.path.eval(t);
            let active = mi == self.active_mask;
            let color = if active { MASK_COLOR } else { MASK_COLOR.gamma_multiply(0.5) };
            line(flatten(&path).into_iter().map(l2s).collect(), Stroke::new(if active { 1.5_f32 } else { 1.0_f32 }, color));
            for (vi, v) in path.vertices.iter().enumerate() {
                let p = l2s(v.p);
                if active {
                    for tan in [v.tan_in, v.tan_out] {
                        if tan != [0.0; 2] {
                            let h = l2s([v.p[0] + tan[0], v.p[1] + tan[1]]);
                            painter.line_segment([p, h], Stroke::new(1.0_f32, color.gamma_multiply(0.7)));
                            painter.circle_filled(h, 3.0, color);
                        }
                    }
                }
                let selected = active && self.active_vertex == Some(vi);
                let r = Rect::from_center_size(p, egui::vec2(6.0, 6.0));
                painter.rect_filled(r, 0.0, if selected { Color32::WHITE } else { color });
                painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::BLACK));
            }
        }

        // 描きかけのペンのパス
        if let Some(pen) = &self.pen {
            let path = BezPath { vertices: pen.vertices.clone(), closed: false };
            let mut pts: Vec<Pos2> = flatten(&path).into_iter().map(l2s).collect();
            if let Some(p) = resp.hover_pos() {
                pts.push(p);
            }
            line(pts, Stroke::new(1.5_f32, Color32::WHITE));
            for (i, v) in pen.vertices.iter().enumerate() {
                let p = l2s(v.p);
                if v.tan_out != [0.0; 2] {
                    painter.line_segment(
                        [l2s([v.p[0] + v.tan_in[0], v.p[1] + v.tan_in[1]]), l2s([v.p[0] + v.tan_out[0], v.p[1] + v.tan_out[1]])],
                        Stroke::new(1.0_f32, Color32::GRAY),
                    );
                }
                // 始点は大きく描く（ここをクリックすると閉じる）
                let size = if i == 0 { 9.0 } else { 6.0 };
                painter.rect_filled(Rect::from_center_size(p, egui::vec2(size, size)), 0.0, Color32::WHITE);
                painter.rect_stroke(Rect::from_center_size(p, egui::vec2(size, size)), 0.0, Stroke::new(1.0_f32, Color32::BLACK));
            }
        }
        // 作成中の矩形・楕円
        if let (Some(ViewerDrag::NewShape { start }), Some(inv), Some(cur)) = (&self.viewer_drag, world.inverse(), pointer) {
            let end = apply(&inv, to_comp(cur));
            if let Some(path) = shape_path(self.tool, *start, end, shift) {
                line(flatten(&path).into_iter().map(l2s).collect(), Stroke::new(1.5_f32, Color32::WHITE));
            }
        }
    }
}

/// 対角の 2 点から矩形・楕円のパスを作る。`square` なら縦横を同じ長さにする。小さすぎれば `None`。
fn shape_path(tool: Tool, a: [f64; 2], b: [f64; 2], square: bool) -> Option<BezPath> {
    let (mut w, mut h) = (b[0] - a[0], b[1] - a[1]);
    if square {
        let s = w.abs().max(h.abs());
        (w, h) = (s * w.signum(), s * h.signum());
    }
    if w.abs() < 1.0 || h.abs() < 1.0 {
        return None;
    }
    let (x, y) = (a[0].min(a[0] + w), a[1].min(a[1] + h));
    Some(match tool {
        Tool::EllipseMask => BezPath::ellipse([x + w.abs() / 2.0, y + h.abs() / 2.0], [w.abs() / 2.0, h.abs() / 2.0]),
        _ => BezPath::rect(x, y, w.abs(), h.abs()),
    })
}
