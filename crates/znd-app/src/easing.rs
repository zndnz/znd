//! イージングのパネル（右側の「イージング」タブ）。
//!
//! - 一覧: よく使うカーブとお気に入りを、形の分かる小さなグラフで並べる。押すと選択中のキーの区間に付く。
//!   ポインタを載せると点が動いて、速さの変化が見える。検索で絞り込める。
//! - 編集: いま選んでいるカーブを、ハンドルのドラッグ・数値・`cubic-bezier(...)` の文字で直せる。
//!   「選択に自動で適用」を入れておくと、触るたびに選択中の区間へ反映する。
//! - 選択中の区間のカーブを拾う（スポイト）、外してハンドルに戻す、お気に入りに保存する。
//! - ショートカット（既定は E）で、いまのカーブを選択に付ける。

use crate::timeline::KeySel;
use crate::{theme, widgets, App};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui};
use serde::{Deserialize, Serialize};
use znd_core::ease::{EaseCurve, EaseDir, CURVE_PRESETS};
use znd_core::keys;

/// お気に入りのカーブ（設定に保存する）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FavoriteEase {
    pub name: String,
    pub curve: EaseCurve,
}

/// パネルの状態。
pub(crate) struct EaseUi {
    /// 編集中・次に付けるカーブ。
    pub current: EaseCurve,
    /// 編集したら選択中の区間へすぐ反映する。
    pub live: bool,
    pub search: String,
    /// 編集欄の `cubic-bezier(...)` の文字。
    css: String,
    /// お気に入りに付ける名前。
    fav_name: String,
}

impl Default for EaseUi {
    fn default() -> Self {
        let current = EaseCurve::bezier(0.33, 0.0, 0.67, 1.0);
        EaseUi { current, live: true, search: String::new(), css: current.css().unwrap_or_default(), fav_name: String::new() }
    }
}

/// カーブの値の範囲（行き過ぎを含めて描くため）。
fn value_range(c: &EaseCurve) -> (f64, f64) {
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for i in 0..=64 {
        let v = c.eval(i as f64 / 64.0);
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if let EaseCurve::Bezier { y1, y2, .. } = c {
        lo = lo.min(*y1).min(*y2);
        hi = hi.max(*y1).max(*y2);
    }
    (lo, hi)
}

/// カーブの形を `rect` に描く。`range` は縦軸の範囲（0..1 を外れるカーブは縮めて収める）。
pub(crate) fn paint_curve(p: &egui::Painter, rect: Rect, c: &EaseCurve, color: Color32, width: f32, range: (f64, f64)) {
    let (lo, hi) = range;
    let to = |u: f64, v: f64| pos2(rect.left() + u as f32 * rect.width(), rect.bottom() - ((v - lo) / (hi - lo).max(1e-6)) as f32 * rect.height());
    // 0 と 1 の目安の線
    let guide = Stroke::new(1.0_f32, Color32::from_white_alpha(18));
    p.line_segment([to(0.0, 0.0), to(1.0, 0.0)], guide);
    p.line_segment([to(0.0, 1.0), to(1.0, 1.0)], guide);
    let n = (rect.width() as usize).clamp(24, 240);
    let pts: Vec<Pos2> = (0..=n).map(|i| i as f64 / n as f64).map(|u| to(u, c.eval(u))).collect();
    p.add(egui::Shape::line(pts, Stroke::new(width, color)));
}

/// ポインタを載せている間、点を動かして速さの変化を見せる（1.2 秒で 1 往復の片道）。
fn paint_motion(ui: &Ui, rect: Rect, c: &EaseCurve, range: (f64, f64)) {
    let t = ui.input(|i| i.time);
    let u = ((t % 1.6) / 1.2).min(1.0);
    let v = c.eval(u);
    let (lo, hi) = range;
    let y = rect.bottom() - ((v - lo) / (hi - lo).max(1e-6)) as f32 * rect.height();
    let p = ui.painter();
    p.circle_filled(pos2(rect.left() + u as f32 * rect.width(), y), 3.0, theme::KEY_SELECTED);
    // 右端の縦の棒の上を、値だけで動く点（実際の動きに近い見え方）
    let x = rect.right() + 5.0;
    p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    p.circle_filled(pos2(x, y), 3.0, Color32::WHITE);
    ui.ctx().request_repaint();
}

impl App {
    /// 選択中のキーにカーブを付ける（`None` で外す）。`coalesce` なら直前の同じ操作と取り消し単位をまとめる。
    pub(crate) fn apply_ease(&mut self, curve: Option<EaseCurve>, coalesce: bool) {
        let sel: Vec<KeySel> = self.selected_keys.clone();
        if sel.is_empty() {
            self.status = "イージングを付けるキーフレームを、タイムラインで 2 つ以上（または区間の始めの 1 つ）選んでください".into();
            return;
        }
        let groups = crate::keyops::by_layer(&sel);
        let mut n = 0;
        let label = if curve.is_some() { "イージング" } else { "イージングを外す" };
        self.edit_comp(label, coalesce, |c| {
            for (id, addrs) in &groups {
                if let Some(l) = c.layer_mut(id).filter(|l| !l.locked) {
                    n += keys::set_curve(l, addrs, curve);
                }
            }
        });
        if !coalesce {
            self.status = match curve {
                Some(c) => format!("{} 区間に{}のイージングを付けました", n, c.kind_label()),
                None => format!("{n} 区間のイージングを外しました（ハンドルの補間に戻ります）"),
            };
        }
    }

    /// 選択中の最初の区間のカーブを、編集中のカーブにする。
    fn pick_ease(&mut self) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        for (id, addrs) in crate::keyops::by_layer(&self.selected_keys) {
            if let Some(c) = comp.layer(&id).and_then(|l| keys::curve_of(l, &addrs)) {
                self.set_current_ease(c);
                self.status = format!("選択中の区間の{}のカーブを拾いました", c.kind_label());
                return;
            }
        }
        self.status = "選択中の区間にはイージングカーブがありません".into();
    }

    fn set_current_ease(&mut self, c: EaseCurve) {
        self.ease.current = c;
        self.ease.css = c.css().unwrap_or_default();
    }

    /// 一覧の 1 枚。押されたら true。
    fn ease_card(ui: &mut Ui, name: &str, c: &EaseCurve, selected: bool) -> egui::Response {
        let size = vec2(66.0, 62.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        if ui.is_rect_visible(rect) {
            let bg = if selected {
                theme::ACCENT.gamma_multiply(0.55)
            } else if resp.hovered() {
                theme::BG_HOVER
            } else {
                theme::BG2
            };
            ui.painter().rect_filled(rect, 3.0, bg);
            let graph = Rect::from_min_size(rect.min + vec2(7.0, 6.0), vec2(46.0, 34.0));
            let range = value_range(c);
            paint_curve(&ui.painter_at(rect), graph, c, if selected { Color32::WHITE } else { theme::TEXT }, 1.5, range);
            if resp.hovered() {
                paint_motion(ui, graph, c, range);
            }
            // 名前は 2 行まで（分類名を省いた短い名前）
            let short = name.rsplit(' ').next().unwrap_or(name);
            let short = if name.contains(' ') { short.to_string() } else { name.to_string() };
            ui.painter().with_clip_rect(rect).text(
                pos2(rect.center().x, rect.bottom() - 10.0),
                Align2::CENTER_CENTER,
                short,
                FontId::proportional(9.5),
                theme::TEXT_DIM,
            );
        }
        resp.on_hover_text(name)
    }

    pub(crate) fn easing_tab(&mut self, ui: &mut Ui) {
        let n_keys = self.selected_keys.len();
        widgets::padded(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let note = if n_keys == 0 { "キーフレームを選ぶと付けられます".to_string() } else { format!("選択中のキーフレーム {n_keys} 個") };
                ui.label(egui::RichText::new(note).size(10.5).color(theme::TEXT_DIM));
            });
            ui.horizontal_wrapped(|ui| {
                let apply_key = self.shortcut_text(crate::keymap::Action::ApplyEase);
                if ui.add_enabled(n_keys > 0, egui::Button::new("選択に適用").shortcut_text(apply_key)).clicked() {
                    let c = self.ease.current;
                    self.apply_ease(Some(c), false);
                }
                if ui.add_enabled(n_keys > 0, egui::Button::new("拾う")).on_hover_text("選択中の区間のカーブを編集欄に読み込みます").clicked() {
                    self.pick_ease();
                }
                if ui.add_enabled(n_keys > 0, egui::Button::new("外す")).on_hover_text("カーブを外して、キーのハンドル（速度・影響度）の補間に戻します").clicked() {
                    self.apply_ease(None, false);
                }
                ui.checkbox(&mut self.ease.live, "触ったらすぐ反映")
                    .on_hover_text("一覧から選んだり編集したりしたとき、選択中の区間にすぐ付けます");
            });
        });
        // よく使う一覧を上に、細かい編集を下に置く
        widgets::section(ui, "一覧", true, |ui| {
            widgets::padded(ui, |ui| {
                let w = ui.available_width() - 22.0;
                widgets::search_box(ui, &mut self.ease.search, "カーブを検索（例: バウンス、アウト）", w);
            });
            self.ease_gallery(ui);
        });
        widgets::section(ui, "カーブを編集", true, |ui| widgets::padded(ui, |ui| self.ease_editor(ui)));
    }

    fn ease_gallery(&mut self, ui: &mut Ui) {
        let mut chosen: Option<EaseCurve> = None;
        let mut remove: Option<usize> = None;
        let search = self.ease.search.trim().to_string();
        let current = self.ease.current;
        let favs = self.prefs.ease_favorites.clone();
        let fav_found: Vec<(usize, &FavoriteEase)> =
            favs.iter().enumerate().filter(|(_, f)| widgets::matches(&search, &format!("お気に入り {}", f.name))).collect();
        if !fav_found.is_empty() {
            ui.label(egui::RichText::new("お気に入り").size(10.0).color(theme::TEXT_DIM));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
                for (i, f) in fav_found {
                    let r = Self::ease_card(ui, &f.name, &f.curve, f.curve == current);
                    if r.clicked() {
                        chosen = Some(f.curve);
                    }
                    r.context_menu(|ui| {
                        if ui.button("お気に入りから外す").clicked() {
                            ui.close_menu();
                            remove = Some(i);
                        }
                    });
                }
            });
        }
        let mut groups: Vec<&str> = Vec::new();
        for p in CURVE_PRESETS {
            if !groups.contains(&p.group) {
                groups.push(p.group);
            }
        }
        for g in groups {
            let found: Vec<_> = CURVE_PRESETS.iter().filter(|p| p.group == g && widgets::matches(&search, &format!("{} {}", p.group, p.name))).collect();
            if found.is_empty() {
                continue;
            }
            ui.label(egui::RichText::new(g).size(10.0).color(theme::TEXT_DIM));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
                for p in found {
                    if Self::ease_card(ui, p.name, &p.curve, p.curve == current).clicked() {
                        chosen = Some(p.curve);
                    }
                }
            });
        }
        if let Some(i) = remove {
            self.prefs.ease_favorites.remove(i);
            self.save_prefs();
        }
        if let Some(c) = chosen {
            self.set_current_ease(c);
            if self.ease.live && !self.selected_keys.is_empty() {
                self.apply_ease(Some(c), false);
            }
        }
    }

    fn save_prefs(&mut self) {
        if let Err(e) = self.prefs.save() {
            self.status = format!("設定を保存できません: {e}");
        }
    }

    fn ease_editor(&mut self, ui: &mut Ui) {
        let before = self.ease.current;
        let mut c = before;
        // 種類
        ui.horizontal(|ui| {
            let kinds = [
                ("ベジェ", matches!(c, EaseCurve::Bezier { .. }), EaseCurve::bezier(0.33, 0.0, 0.67, 1.0)),
                ("振動", matches!(c, EaseCurve::Elastic { .. }), EaseCurve::elastic(EaseDir::Out)),
                ("バウンス", matches!(c, EaseCurve::Bounce { .. }), EaseCurve::bounce(EaseDir::Out)),
                ("階段", matches!(c, EaseCurve::Steps { .. }), EaseCurve::Steps { count: 4 }),
            ];
            for (label, on, init) in kinds {
                if ui.selectable_label(on, label).clicked() && !on {
                    c = init;
                }
            }
        });
        // グラフ（ベジェはハンドルをドラッグできる）
        let side = ui.available_width().min(220.0) - 16.0;
        let (outer, _) = ui.allocate_exact_size(vec2(side + 16.0, side * 0.8 + 8.0), Sense::hover());
        let graph = Rect::from_min_size(outer.min + vec2(4.0, 4.0), vec2(side, side * 0.8));
        let p = ui.painter_at(outer);
        p.rect_filled(graph.expand(2.0), 2.0, theme::INPUT);
        let range = {
            let (lo, hi) = value_range(&c);
            (lo.min(-0.1), hi.max(1.1))
        };
        let to = |u: f64, v: f64| pos2(graph.left() + u as f32 * graph.width(), graph.bottom() - ((v - range.0) / (range.1 - range.0)) as f32 * graph.height());
        let from = |pt: Pos2| {
            (((pt.x - graph.left()) / graph.width()) as f64, range.0 + ((graph.bottom() - pt.y) / graph.height()) as f64 * (range.1 - range.0))
        };
        // 4 分割のグリッド
        for k in 1..4 {
            let f = k as f64 / 4.0;
            let s = Stroke::new(1.0_f32, Color32::from_white_alpha(10));
            p.line_segment([to(f, range.0), to(f, range.1)], s);
            p.line_segment([to(0.0, f), to(1.0, f)], s);
        }
        paint_curve(&p, graph, &c, theme::TEXT, 2.0, range);
        if let EaseCurve::Bezier { x1, y1, x2, y2 } = &mut c {
            let handles = [(0usize, (0.0, 0.0)), (1usize, (1.0, 1.0))];
            for (h, anchor) in handles {
                let (hx, hy) = if h == 0 { (*x1, *y1) } else { (*x2, *y2) };
                let hp = to(hx, hy);
                p.line_segment([to(anchor.0, anchor.1), hp], Stroke::new(1.0_f32, theme::KEY_SELECTED.gamma_multiply(0.7)));
                let id = ui.id().with(("ease-handle", h));
                let r = ui.interact(Rect::from_center_size(hp, vec2(14.0, 14.0)), id, Sense::drag());
                p.circle_filled(hp, if r.hovered() || r.dragged() { 6.0 } else { 4.5 }, theme::KEY_SELECTED);
                if r.dragged() {
                    if let Some(pt) = ui.input(|i| i.pointer.interact_pos()) {
                        let (mut u, mut v) = from(pt);
                        // Shift で 0.05 刻みに吸着
                        if ui.input(|i| i.modifiers.shift) {
                            u = (u * 20.0).round() / 20.0;
                            v = (v * 20.0).round() / 20.0;
                        }
                        let u = u.clamp(0.0, 1.0);
                        let v = v.clamp(-4.0, 5.0);
                        if h == 0 {
                            (*x1, *y1) = (u, v);
                        } else {
                            (*x2, *y2) = (u, v);
                        }
                    }
                }
            }
        }
        if ui.rect_contains_pointer(graph) {
            paint_motion(ui, graph, &c, range);
        }
        ui.label(egui::RichText::new("ハンドルをドラッグ（Shift で 0.05 刻み）").size(9.5).color(theme::TEXT_DIM));
        // 数値
        match &mut c {
            EaseCurve::Bezier { x1, y1, x2, y2 } => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    ui.add(egui::DragValue::new(x1).range(0.0..=1.0).speed(0.005).fixed_decimals(2).prefix("x1 "));
                    ui.add(egui::DragValue::new(y1).range(-4.0..=5.0).speed(0.005).fixed_decimals(2).prefix("y1 "));
                    ui.add(egui::DragValue::new(x2).range(0.0..=1.0).speed(0.005).fixed_decimals(2).prefix("x2 "));
                    ui.add(egui::DragValue::new(y2).range(-4.0..=5.0).speed(0.005).fixed_decimals(2).prefix("y2 "));
                });
            }
            EaseCurve::Elastic { dir, amplitude, frequency, decay } => {
                dir_buttons(ui, dir);
                ui.add(egui::Slider::new(amplitude, 0.0..=4.0).text("行き過ぎ"));
                ui.add(egui::Slider::new(frequency, 0.5..=12.0).text("振動の回数"));
                ui.add(egui::Slider::new(decay, 0.0..=20.0).text("減衰"));
            }
            EaseCurve::Bounce { dir, bounces, restitution } => {
                dir_buttons(ui, dir);
                ui.add(egui::Slider::new(bounces, 1..=10).text("跳ねる回数"));
                ui.add(egui::Slider::new(restitution, 0.1..=0.9).text("反発"));
            }
            EaseCurve::Steps { count } => {
                ui.add(egui::Slider::new(count, 1..=60).text("段数"));
            }
        }
        // cubic-bezier の文字（他のツールとのやりとり用）
        if matches!(c, EaseCurve::Bezier { .. }) {
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.ease.css).desired_width(ui.available_width() - 60.0).font(egui::TextStyle::Monospace));
                if r.lost_focus() || r.changed() {
                    if let Some(parsed) = EaseCurve::parse_css(&self.ease.css) {
                        c = parsed;
                    }
                }
                if ui.small_button("コピー").on_hover_text("cubic-bezier(...) の書式でコピーします").clicked() {
                    if let Some(t) = c.css() {
                        ui.ctx().copy_text(t);
                    }
                }
            });
        }
        // お気に入り
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.ease.fav_name).hint_text("名前").desired_width(ui.available_width() - 110.0));
            if ui.button("お気に入りに保存").clicked() {
                let name = if self.ease.fav_name.trim().is_empty() {
                    format!("{} {}", c.kind_label(), self.prefs.ease_favorites.len() + 1)
                } else {
                    self.ease.fav_name.trim().to_string()
                };
                self.prefs.ease_favorites.push(FavoriteEase { name, curve: c });
                self.ease.fav_name.clear();
                self.save_prefs();
            }
        });
        if c != before {
            let c = c.sanitized();
            self.ease.current = c;
            if !ui.memory(|m| m.has_focus(ui.id().with("css"))) {
                if let Some(t) = c.css() {
                    // 文字欄を編集中でなければ、文字も今の値に合わせる
                    if EaseCurve::parse_css(&self.ease.css) != Some(c) {
                        self.ease.css = t;
                    }
                }
            }
            if self.ease.live && !self.selected_keys.is_empty() {
                // ドラッグ中の連続した変更は 1 回の取り消しにまとめる
                self.apply_ease(Some(c), true);
            }
        }
    }
}

impl App {
    /// キーの右クリック・グラフエディターに出す、カーブの一覧（形の絵付き）。
    pub(crate) fn ease_menu(&mut self, ui: &mut Ui) {
        ui.set_min_width(220.0);
        let current = self.ease.current;
        let apply = self.shortcut_text(crate::keymap::Action::ApplyEase);
        let mut chosen: Option<Option<EaseCurve>> = None;
        if ui.add(egui::Button::new(format!("いまのカーブ（{}）", current.kind_label())).shortcut_text(apply)).clicked() {
            chosen = Some(Some(current));
        }
        if ui.button("カーブを外す（ハンドルに戻す）").clicked() {
            chosen = Some(None);
        }
        if ui.button("イージングのパネルを開く").clicked() {
            ui.close_menu();
            self.right_tab = 2;
        }
        ui.separator();
        let favs = self.prefs.ease_favorites.clone();
        let items = favs.iter().map(|f| (f.name.as_str(), f.curve)).chain(CURVE_PRESETS.iter().map(|p| (p.name, p.curve)));
        let mut pick = widgets::pick_search(ui, ui.make_persistent_id("ease-menu"), 99);
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for (name, c) in items {
                if !pick.matches(name) {
                    continue;
                }
                let enter = pick.take_enter();
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 2.0, theme::BG_HOVER);
                }
                let g = Rect::from_min_size(rect.min + vec2(4.0, 3.0), vec2(26.0, 16.0));
                paint_curve(ui.painter(), g, &c, theme::TEXT, 1.2, value_range(&c));
                ui.painter().text(pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, name, FontId::proportional(11.0), theme::TEXT);
                if resp.clicked() || enter {
                    chosen = Some(Some(c));
                }
            }
        });
        if let Some(c) = chosen {
            ui.close_menu();
            if let Some(c) = c {
                self.set_current_ease(c);
            }
            self.apply_ease(c, false);
        }
    }
}

fn dir_buttons(ui: &mut Ui, dir: &mut EaseDir) {
    ui.horizontal(|ui| {
        for d in EaseDir::ALL {
            ui.selectable_value(dir, d, d.label());
        }
    });
}
