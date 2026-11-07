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
        ui.painter().rect_filled(rect.shrink(2.0), 5.0, bg);
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

/// 32×30 のアイコンボタン。`on` なら選択色で塗る。
pub(crate) fn icon_toggle(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 30.0), Sense::click());
    paint_icon_button(ui, rect, &resp, icon, on);
    resp.on_hover_text(tip)
}

pub(crate) fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    icon_toggle(ui, icon, false, tip)
}

/// 行の中に置く 24px 角の小さなアイコンボタン。
pub(crate) fn small_icon(ui: &mut Ui, icon: Icon, on: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
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
        ui.painter().rect_filled(rect, 4.0, theme::BG_HOVER);
    }
    icons::paint_sized(ui.painter(), rect, icon, color, 16.0);
    resp.on_hover_text(tip)
}

/// 決まった位置に置くアイコンのスイッチ（タイムラインの列）。オフのときは `off` の色で薄く描く。
pub(crate) fn icon_cell(ui: &Ui, rect: Rect, id: Id, icon: Icon, on: bool, off: Color32, tip: &str) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let color = if on { theme::TEXT } else { off };
    let color = if resp.hovered() { Color32::from_gray(0xf2) } else { color };
    icons::paint_sized(ui.painter(), rect, icon, color, 15.0);
    resp.on_hover_text(tip)
}

/// 縦の区切り線（ツールバー用）。
pub(crate) fn vsep(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(17.0, 18.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0_f32, theme::LINE));
}

/// パネル上端のタブ列。選択中のタブは文字を明るくし、下に色の線を引くだけの控えめな見た目。右側の余白の矩形を返す。
pub(crate) fn tab_strip(ui: &mut Ui, labels: &[&str], selected: &mut usize) -> Rect {
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::TAB_H), Sense::hover());
    let p = ui.painter_at(strip);
    p.rect_filled(strip, 0.0, theme::BG1);
    p.hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0_f32, theme::LINE));
    let font = FontId::proportional(13.0);
    let mut x = strip.left() + 6.0;
    for (i, label) in labels.iter().enumerate() {
        let galley = p.layout_no_wrap(label.to_string(), font.clone(), theme::TEXT);
        let w = (galley.size().x + 24.0).min((strip.right() - x).max(0.0));
        if w < 32.0 {
            break;
        }
        let rect = Rect::from_min_size(pos2(x, strip.top()), vec2(w, theme::TAB_H));
        let resp = ui.interact(rect, ui.id().with(("tab", i)), Sense::click());
        if resp.clicked() {
            *selected = i;
        }
        let on = *selected == i;
        if on {
            let line = Rect::from_min_max(pos2(rect.left() + 8.0, rect.bottom() - 2.0), pos2(rect.right() - 8.0, rect.bottom()));
            p.rect_filled(line, 1.0, theme::ACCENT_HI);
        }
        let color = if on {
            Color32::from_gray(0xf0)
        } else if resp.hovered() {
            Color32::from_gray(0xc0)
        } else {
            theme::TEXT_DIM
        };
        p.with_clip_rect(rect).text(rect.center(), Align2::CENTER_CENTER, label, font.clone(), color);
        x += w;
    }
    Rect::from_min_max(pos2(x, strip.top()), strip.right_bottom())
}

/// 折りたたみできる見出し付きの区画。見出しはパネルの幅いっぱいの帯。
pub(crate) fn section<R>(ui: &mut Ui, title: &str, default_open: bool, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let id = ui.make_persistent_id(("section", title));
    let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, default_open));
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let p = ui.painter_at(rect);
    if resp.hovered() {
        p.rect_filled(rect, 0.0, theme::BG2);
    }
    p.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0_f32, theme::LINE));
    let twirl = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 8.0), vec2(16.0, 16.0));
    icons::paint_sized(&p, twirl, if open { Icon::TwirlOpen } else { Icon::TwirlClosed }, theme::TEXT_DIM, 13.0);
    p.text(pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, title, FontId::proportional(13.0), Color32::from_gray(0xe4));
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    open.then(|| egui::Frame::none().inner_margin(egui::Margin { left: 14.0, right: 12.0, top: 2.0, bottom: 12.0 }).show(ui, add).inner)
}

/// パネル本文の余白（見出しの帯は幅いっぱい、本文はここに入れる）。
pub(crate) fn padded<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none().inner_margin(egui::Margin { left: 14.0, right: 12.0, top: 10.0, bottom: 10.0 }).show(ui, add).inner
}

/// 決まった矩形に置く、枠のないドロップダウン（タイムラインのモード・マット・親の列）。
/// `n` は項目の数（多いときだけ先頭に検索欄を出す）。
pub(crate) fn cell_menu(ui: &Ui, rect: Rect, id: Id, text: &str, n: usize, add: impl FnOnce(&mut Ui, &mut Pick)) {
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
        FontId::proportional(12.0),
        color,
    );
    let c = pos2(inner.right() - 5.5, inner.center().y);
    p.add(egui::Shape::convex_polygon(vec![c + vec2(-2.5, -1.5), c + vec2(2.5, -1.5), c + vec2(0.0, 2.0)], theme::TEXT_DIM, Stroke::NONE));
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    let close = if n >= SEARCH_MIN { egui::PopupCloseBehavior::CloseOnClickOutside } else { egui::PopupCloseBehavior::CloseOnClick };
    egui::popup::popup_below_widget(ui, popup, &resp, close, |ui| {
        ui.set_min_width(130.0);
        let mut pick = pick_search(ui, id, n);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            add(ui, &mut pick);
            pick.empty_note(ui);
        });
    });
}

/// 虫眼鏡付きの検索欄。
pub(crate) fn search_box(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 26.0), Sense::hover());
        icons::paint_sized(ui.painter(), rect, Icon::Search, theme::TEXT_DIM, 15.0);
        ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(width))
    })
    .inner
}

/// 何も無いパネルに出す案内文。
pub(crate) fn hint(ui: &mut Ui, text: &str) {
    padded(ui, |ui| {
        ui.label(egui::RichText::new(text).color(theme::TEXT_DIM).size(12.5));
    });
}

// ---------- 検索できる選択欄 ----------

/// これ以上の項目がある一覧には、先頭に検索欄を出す（数個の一覧では邪魔になるだけなので出さない）。
pub(crate) const SEARCH_MIN: usize = 8;

/// 検索の比較用に文字を揃える（大文字小文字・全角半角・カタカナとひらがなの違いを無視する）。
pub(crate) fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            // 全角英数・記号 → 半角
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{3000}' => ' ',
            // カタカナ → ひらがな
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// `query` を空白で区切った語がすべて `label` に含まれるか（空の検索はすべてに一致する）。
pub(crate) fn matches(query: &str, label: &str) -> bool {
    let label = fold(label);
    fold(query).split_whitespace().all(|w| label.contains(w))
}

/// 選択欄の中身を組み立てるときに渡す、検索の状態。項目は `item` などで足す（検索に合わないものは出さない）。
pub(crate) struct Pick {
    query: String,
    /// 検索欄で Enter が押された（最初に一致した項目を選ぶ）。
    enter: bool,
    /// 表示した項目の数。
    pub shown: usize,
    /// 一覧を開いた直後（選択中の項目までスクロールする）。
    opened: bool,
}

impl Pick {
    fn new(query: String, enter: bool, opened: bool) -> Self {
        Pick { query, enter, shown: 0, opened }
    }

    fn reveal(&self, selected: bool, r: &Response) {
        if selected && self.opened {
            r.scroll_to_me(Some(egui::Align::Center));
        }
    }

    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn matches(&self, label: &str) -> bool {
        matches(&self.query, label)
    }

    /// 選んだら一覧を閉じる。
    fn finish(ui: &Ui, clicked: bool) -> bool {
        if clicked {
            ui.memory_mut(|m| m.close_popup());
        }
        clicked
    }

    /// 検索欄で Enter が押されたか（1 回だけ true を返す。最初に一致した項目を選ぶのに使う）。
    pub fn take_enter(&mut self) -> bool {
        std::mem::take(&mut self.enter) && self.searching()
    }

    /// 1 行の項目。押されたら true。
    pub fn item(&mut self, ui: &mut Ui, selected: bool, label: impl AsRef<str>) -> bool {
        self.item_tip(ui, selected, label.as_ref(), "")
    }

    /// ポインタを載せると説明が出る項目。
    pub fn item_tip(&mut self, ui: &mut Ui, selected: bool, label: &str, tip: &str) -> bool {
        if !self.matches(label) {
            return false;
        }
        self.shown += 1;
        let mut r = ui.selectable_label(selected, label);
        if !tip.is_empty() {
            r = r.on_hover_text(tip);
        }
        self.reveal(selected, &r);
        let enter = self.take_enter();
        Self::finish(ui, r.clicked() || enter)
    }

    /// `ui.selectable_value` と同じ。選んだ値を `cur` に入れ、変わったら true。
    pub fn value<T: PartialEq>(&mut self, ui: &mut Ui, cur: &mut T, value: T, label: impl AsRef<str>) -> bool {
        if self.item(ui, *cur == value, label) && *cur != value {
            *cur = value;
            return true;
        }
        false
    }

    /// 絵を描く項目（フォントの見本など）。`keys` は検索で照らし合わせる文字列、`paint` は行の矩形に描く。
    pub fn custom(&mut self, ui: &mut Ui, selected: bool, keys: &str, height: f32, paint: impl FnOnce(&egui::Painter, Rect)) -> bool {
        if !self.matches(keys) {
            return false;
        }
        self.shown += 1;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), height), Sense::click());
        if ui.is_rect_visible(rect) {
            let bg = if selected {
                theme::ACCENT
            } else if resp.hovered() {
                theme::BG_HOVER
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 2.0, bg);
            paint(&ui.painter_at(rect), rect);
        }
        self.reveal(selected, &resp);
        let enter = self.take_enter();
        Self::finish(ui, resp.clicked() || enter)
    }

    /// 区切りの見出し（検索中は出さない）。
    pub fn heading(&self, ui: &mut Ui, text: &str) {
        if !self.searching() {
            ui.separator();
            ui.weak(text);
        }
    }

    /// 一致するものが無かったときの案内。
    fn empty_note(&self, ui: &mut Ui) {
        if self.shown == 0 && self.searching() {
            ui.weak("一致する項目がありません");
        }
    }
}

/// 開いた一覧の先頭に置く検索欄。開いた直後に入力を受け付け、閉じたら中身を消す。
pub(crate) fn pick_search(ui: &mut Ui, id: Id, n: usize) -> Pick {
    let qid = id.with("pick-query");
    let now = ui.ctx().cumulative_pass_nr();
    let (mut query, last) = ui.data_mut(|d| d.get_temp::<(String, u64)>(qid)).unwrap_or_default();
    let opened = last + 1 < now;
    if n < SEARCH_MIN {
        ui.data_mut(|d| d.insert_temp(qid, (String::new(), now)));
        return Pick::new(String::new(), false, opened);
    }
    if opened {
        query.clear();
    }
    let resp = search_box(ui, &mut query, "検索", 200.0);
    if opened {
        resp.request_focus();
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if enter {
        // Enter で選んだあとも検索欄に戻れるように
        resp.request_focus();
    }
    ui.data_mut(|d| d.insert_temp(qid, (query.clone(), now)));
    Pick::new(query, enter, opened)
}

/// 検索できるドロップダウン（`egui::ComboBox` の代わり）。`n` は項目の数（多いときだけ検索欄を出す）。
/// `width` が 0 なら中身に合わせる。
pub(crate) fn combo(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash,
    width: f32,
    selected: impl Into<egui::WidgetText>,
    n: usize,
    add: impl FnOnce(&mut Ui, &mut Pick),
) -> Response {
    let id = ui.make_persistent_id(&id_salt);
    let popup = id.with("popup");
    let open = ui.memory(|m| m.is_popup_open(popup));
    let h = ui.spacing().interact_size.y;
    let text: egui::WidgetText = selected.into();
    let galley = text.into_galley(ui, Some(egui::TextWrapMode::Truncate), (width - 22.0).max(40.0), egui::TextStyle::Button);
    let w = if width > 0.0 { width } else { galley.size().x + 26.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact_selectable(&resp, open);
        ui.painter().rect(rect, v.rounding, v.weak_bg_fill, v.bg_stroke);
        let clip = Rect::from_min_max(rect.min, pos2(rect.right() - 16.0, rect.bottom()));
        ui.painter().with_clip_rect(clip).galley(pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0), galley, v.text_color());
        let c = pos2(rect.right() - 9.0, rect.center().y);
        ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(-3.5, -2.0), c + vec2(3.5, -2.0), c + vec2(0.0, 2.5)], v.text_color(), Stroke::NONE));
    }
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup));
    }
    // 検索欄を押しても閉じないように（項目を選ぶと Pick が閉じる）
    let close = if n >= SEARCH_MIN { egui::PopupCloseBehavior::CloseOnClickOutside } else { egui::PopupCloseBehavior::CloseOnClick };
    egui::popup::popup_below_widget(ui, popup, &resp, close, |ui| {
        ui.set_min_width(w.max(120.0));
        let mut pick = pick_search(ui, id, n);
        egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
            add(ui, &mut pick);
            pick.empty_note(ui);
        });
    });
    resp
}
