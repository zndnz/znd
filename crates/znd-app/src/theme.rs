//! 配色と寸法。ニュートラルなダーク UI。境界線は最小限にし、文字と余白を大きめに取ってゆったり見せる。

use eframe::egui::{self, Color32, FontFamily, FontId, Rounding, Stroke, TextStyle};

const fn gray(v: u8) -> Color32 {
    Color32::from_rgb(v, v, v)
}

/// タイムラインのレーン・タブ列など、いちばん奥の面。
pub const BG0: Color32 = gray(0x1d);
/// パネルの面。
pub const BG1: Color32 = gray(0x20);
/// 見出し・列ヘッダー。
pub const BG2: Color32 = gray(0x25);
/// ボタン・選択中のタブ。
pub const BG3: Color32 = gray(0x2b);
pub const BG_HOVER: Color32 = gray(0x34);
pub const ROW_SELECTED: Color32 = gray(0x2e);
pub const LINE: Color32 = gray(0x33);
pub const LINE_HI: Color32 = gray(0x3a);
pub const INPUT: Color32 = gray(0x18);
pub const TEXT: Color32 = gray(0xd0);
pub const TEXT_DIM: Color32 = gray(0x90);
pub const TEXT_OFF: Color32 = gray(0x55);
pub const ACCENT: Color32 = Color32::from_rgb(0x3b, 0x6d, 0x92);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0x47, 0x78, 0x9d);
/// 数値・タイムコード。
pub const VALUE: Color32 = Color32::from_rgb(0x6f, 0xa8, 0xd6);
/// 現在時刻インジケーター。
pub const PLAYHEAD: Color32 = Color32::from_rgb(0x58, 0xa6, 0xe8);
pub const KEY_SELECTED: Color32 = Color32::from_rgb(0x7c, 0xbc, 0xf0);
pub const WARN: Color32 = Color32::from_rgb(0xd8, 0xb0, 0x55);
pub const ERROR: Color32 = Color32::from_rgb(0xe0, 0x70, 0x70);
pub const OK: Color32 = Color32::from_rgb(0x62, 0xb0, 0x7c);
pub const CACHE: Color32 = Color32::from_rgb(0x3f, 0x9a, 0x5c);
pub const BEAT: Color32 = Color32::from_rgb(0xc8, 0xb0, 0x70);

pub const TAB_H: f32 = 32.0;
pub const ROW_H: f32 = 26.0;
/// 縦スクロールバーが占める幅（`apply` の設定と揃える）。
pub const SCROLL_W: f32 = 10.0;

/// レイヤーのカラーラベル。識別のためだけの色なので彩度は低い。
pub const LABELS: [(&str, Color32); 12] = [
    ("レッド", Color32::from_rgb(0x9a, 0x50, 0x50)),
    ("イエロー", Color32::from_rgb(0xa8, 0x98, 0x52)),
    ("アクア", Color32::from_rgb(0x58, 0x98, 0x9c)),
    ("ピンク", Color32::from_rgb(0xa4, 0x6c, 0x88)),
    ("ラベンダー", Color32::from_rgb(0x7e, 0x78, 0xac)),
    ("ピーチ", Color32::from_rgb(0xae, 0x84, 0x68)),
    ("シーフォーム", Color32::from_rgb(0x6c, 0xa0, 0x84)),
    ("ブルー", Color32::from_rgb(0x52, 0x74, 0xa8)),
    ("グリーン", Color32::from_rgb(0x64, 0x90, 0x54)),
    ("パープル", Color32::from_rgb(0x82, 0x5c, 0x9c)),
    ("オレンジ", Color32::from_rgb(0xb0, 0x7c, 0x48)),
    ("ブラウン", Color32::from_rgb(0x84, 0x68, 0x50)),
];

/// ラベル番号（1 始まり）の色。0（未設定）は ID から決めた色にして、隣り合うレイヤーを見分けやすくする。
pub fn label_color(label: u8, id: &str) -> Color32 {
    let i = match label {
        0 => id.bytes().fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % LABELS.len(),
        n => (n as usize - 1) % LABELS.len(),
    };
    LABELS[i].1
}

pub fn panel_frame() -> egui::Frame {
    egui::Frame::none().fill(BG1)
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 6.0);
    s.button_padding = egui::vec2(10.0, 4.0);
    s.interact_size = egui::vec2(26.0, 26.0);
    s.indent = 16.0;
    s.icon_width = 15.0;
    s.icon_width_inner = 8.0;
    s.icon_spacing = 6.0;
    s.combo_width = 80.0;
    s.text_edit_width = 160.0;
    s.slider_width = 110.0;
    s.menu_margin = egui::Margin::same(6.0);
    s.window_margin = egui::Margin::same(12.0);
    s.scroll = egui::style::ScrollStyle {
        floating: false,
        bar_width: SCROLL_W - 1.0,
        bar_inner_margin: 1.0,
        bar_outer_margin: 0.0,
        handle_min_length: 24.0,
        foreground_color: false,
        ..egui::style::ScrollStyle::solid()
    };

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = BG1;
    v.window_fill = BG2;
    v.window_stroke = Stroke::new(1.0_f32, LINE_HI);
    v.window_rounding = Rounding::same(6.0);
    v.menu_rounding = Rounding::same(6.0);
    v.window_shadow = egui::Shadow { offset: egui::vec2(0.0, 4.0), blur: 12.0, spread: 0.0, color: Color32::from_black_alpha(90) };
    v.popup_shadow = egui::Shadow { offset: egui::vec2(0.0, 3.0), blur: 8.0, spread: 0.0, color: Color32::from_black_alpha(80) };
    v.extreme_bg_color = INPUT;
    v.faint_bg_color = BG2;
    v.code_bg_color = INPUT;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERROR;
    v.hyperlink_color = VALUE;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0_f32, Color32::from_gray(0xf0));
    v.slider_trailing_fill = true;
    v.indent_has_left_vline = false;
    v.clip_rect_margin = 0.0;

    let r = Rounding::same(4.0);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BG1;
    w.noninteractive.weak_bg_fill = BG1;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.noninteractive.rounding = r;
    w.inactive.bg_fill = BG3;
    w.inactive.weak_bg_fill = BG3;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.inactive.rounding = r;
    w.hovered.bg_fill = BG_HOVER;
    w.hovered.weak_bg_fill = BG_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, gray(0x4a));
    w.hovered.fg_stroke = Stroke::new(1.0_f32, gray(0xf0));
    w.hovered.rounding = r;
    w.active.bg_fill = ACCENT;
    w.active.weak_bg_fill = ACCENT;
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_HI);
    w.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    w.active.rounding = r;
    w.open.bg_fill = BG3;
    w.open.weak_bg_fill = BG_HOVER;
    w.open.bg_stroke = Stroke::new(1.0_f32, LINE_HI);
    w.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    w.open.rounding = r;
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.expansion = 0.0;
    }
    ctx.set_style(style);
}
