//! パネルを組み立てる小さな部品: アイコンボタン、タブ列、折りたたみ見出し、セル型のドロップダウン。

use crate::icons::{self, Icon};
use crate::theme;
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Id, Rect, Response, Sense, Stroke, Ui};

fn paint_icon_button(ui: &Ui, rect: Rect, resp: &Response, icon: Icon, on: bool) {
    let bg = if on {
        theme::ACCENT
    } else if resp.is_pointer_button_down_on() {
        theme::BG0
    } else if resp.hovered() {
        theme::BG_HOVER
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect.shrink(1.0), 2.0, bg);
    }
    let color = if !ui.is_enabled() {
        theme::TEXT_OFF
    } else if on || resp.hovered() {
        Color32::from_gray(0xf2)
    } else {
        theme::TEXT
    };
    icons::paint(ui.painter(), rect, icon, color);
}

/// 24×22 のアイコンボタン。`on` なら選択色で塗る。
pub(crate) fn icon_toggle(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
    paint_icon_button(ui, rect, &resp, icon, on);
    resp.on_hover_text(tip)
}

pub(crate) fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    icon_toggle(ui, icon, false, tip)
}

/// 行の中に置く 18px 角の小さなアイコンボタン。
pub(crate) fn small_icon(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let color = if !ui.is_enabled() {
        theme::TEXT_OFF
    } else if on {
        theme::KEY_SELECTED
    } else if resp.hovered() {
        Color32::from_gray(0xf2)
    } else {
        theme::TEXT_DIM
    };
    if resp.hovered() {
        ui.painter().rect_filled(rect, 2.0, theme::BG_HOVER);
    }
    icons::paint_sized(ui.painter(), rect, icon, color, 14.0);
    resp.on_hover_text(tip)
}

/// 決まった位置に置くアイコンのスイッチ（タイムラインの列）。オフのときは `off` の色で薄く描く。
pub(crate) fn icon_cell(ui: &Ui, rect: Rect, id: Id, icon: Icon, on: bool, off: Color32, tip: &str) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let color = if on { theme::TEXT } else { off };
    let color = if resp.hovered() { Color32::from_gray(0xf2) } else { color };
    icons::paint_sized(ui.painter(), rect, icon, color, 13.0);
    resp.on_hover_text(tip)
}

/// 縦の区切り線（ツールバー用）。
pub(crate) fn vsep(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(7.0, 18.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0_f32, theme::LINE_HI));
}

/// パネル上端のタブ列。選択中のタブだけパネルと同じ明るさにする。右側の余白の矩形を返す。
pub(crate) fn tab_strip(ui: &mut Ui, labels: &[&str], selected: &mut usize) -> Rect {
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::TAB_H), Sense::hover());
    let p = ui.painter_at(strip);
    p.rect_filled(strip, 0.0, theme::BG0);
    p.hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0_f32, theme::LINE));
    let font = FontId::proportional(11.0);
    let mut x = strip.left();
    for (i, label) in labels.iter().enumerate() {
        let galley = p.layout_no_wrap(label.to_string(), font.clone(), theme::TEXT);
        let w = (galley.size().x + 18.0).min((strip.right() - x).max(0.0));
        if w < 24.0 {
            break;
        }
        let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(w, theme::TAB_H));
        let resp = ui.interact(rect, ui.id().with(("tab", i)), Sense::click());
        if resp.clicked() {
            *selected = i;
        }
        let on = *selected == i;
        if on {
            p.rect_filled(rect, 0.0, theme::BG1);
            p.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0_f32, theme::ACCENT_HI));
        }
        p.vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0_f32, theme::LINE));
        let color = if on {
            theme::TEXT
        } else if resp.hovered() {
            Color32::from_gray(0xb8)
        } else {
            theme::TEXT_DIM
        };
        p.with_clip_rect(rect.shrink2(vec2(6.0, 0.0))).text(
            pos2(rect.left() + 9.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font.clone(),
            color,
        );
        x += w;
    }
    Rect::from_min_max(pos2(x, strip.top()), strip.right_bottom())
}

/// 折りたたみできる見出し付きの区画。見出しはパネルの幅いっぱいの帯。
pub(crate) fn section<R>(ui: &mut Ui, title: &str, default_open: bool, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let id = ui.make_persistent_id(("section", title));
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, default_open));
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 19.0), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, if resp.hovered() { theme::BG3 } else { theme::BG2 });
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0_f32, theme::LINE));
    let twirl = Rect::from_min_size(rect.left_top() + vec2(2.0, 2.0), vec2(14.0, 14.0));
    icons::paint_sized(&p, twirl, if open { Icon::TwirlOpen } else { Icon::TwirlClosed }, theme::TEXT_DIM, 12.0);
    p.text(pos2(rect.left() + 18.0, rect.center().y), Align2::LEFT_CENTER, title, FontId::proportional(11.0), theme::TEXT);
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    open.then(|| egui::Frame::none().inner_margin(egui::Margin { left: 6.0, right: 4.0, top: 3.0, bottom: 4.0 }).show(ui, add).inner)
}

/// パネル本文の余白（見出しの帯は幅いっぱい、本文はここに入れる）。
pub(crate) fn padded<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none().inner_margin(egui::Margin { left: 6.0, right: 4.0, top: 4.0, bottom: 4.0 }).show(ui, add).inner
}

/// 決まった矩形に置く、枠のないドロップダウン（タイムラインのモード・マット・親の列）。
pub(crate) fn cell_menu(ui: &Ui, rect: Rect, id: Id, text: &str, add: impl FnOnce(&mut Ui)) {
    let resp = ui.interact(rect, id, Sense::click());
    let popup = id.with("popup");
    let open = ui.memory(|m| m.is_popup_open(popup));
    let inner = rect.shrink2(vec2(1.0, 2.0));
    let p = ui.painter_at(rect);
    p.rect_filled(inner, 1.0, if open || resp.hovered() { theme::BG_HOVER } else { theme::BG3 });
    let color = if text == "なし" || text == "通常" { theme::TEXT_DIM } else { theme::TEXT };
    p.with_clip_rect(Rect::from_min_max(inner.min, pos2(inner.right() - 9.0, inner.bottom()))).text(
        pos2(inner.left() + 4.0, inner.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(10.5),
        color,
    );
    let c = pos2(inner.right() - 5.5, inner.center().y);
    p.add(egui::Shape::convex_polygon(vec![c + vec2(-2.5, -1.5), c + vec2(2.5, -1.5), c + vec2(0.0, 2.0)], theme::TEXT_DIM, Stroke::NONE));
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    egui::popup::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(130.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, add);
    });
}

/// 虫眼鏡付きの検索欄。
pub(crate) fn search_box(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 20.0), Sense::hover());
        icons::paint_sized(ui.painter(), rect, Icon::Search, theme::TEXT_DIM, 13.0);
        ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(width))
    })
    .inner
}

/// 何も無いパネルに出す案内文。
pub(crate) fn hint(ui: &mut Ui, text: &str) {
    padded(ui, |ui| {
        ui.label(egui::RichText::new(text).color(theme::TEXT_DIM).size(11.0));
    });
}
