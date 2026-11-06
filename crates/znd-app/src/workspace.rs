//! 画面全体の配置。最上段の 1 本にメニュー・ツール・状態表示、中段に プロジェクト｜コンポジション｜プロパティ、下段にタイムライン。
//!
//! 同じ操作や情報を二か所に置かない。ポインタの下の色・RAM プレビューの量などはビューアの下の 1 行に、
//! 整列とイン／アウトはプロパティの先頭の 1 行にまとめている。パネルの境界はドラッグで動かせる。

use crate::icons::{self, Icon};
use crate::panels::NewLayer;
use crate::timeline::timecode;
use crate::tools::Tool;
use crate::{resolution_label, theme, widgets, App};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, Sense, Stroke};
use znd_audio::SAMPLE_RATE;
use znd_core::effect::all_effects;
use znd_core::eval::world_matrix;
use znd_core::*;

/// プロジェクトパネルの一覧の 1 行。
enum ItemId {
    Comp(CompId),
    Footage(FootageId),
}

struct Item {
    id: ItemId,
    icon: Icon,
    name: String,
    kind: &'static str,
    size: String,
    fps: String,
    duration: String,
    selected: bool,
    warn: bool,
    locked: bool,
}

fn bar_frame(fill: Color32, margin_x: f32) -> egui::Frame {
    egui::Frame::none().fill(fill).inner_margin(egui::Margin::symmetric(margin_x, 0.0))
}

impl App {
    pub(crate) fn workspace(&mut self, ctx: &egui::Context) {
        // 最上段の 1 本にメニュー・ツール・状態表示をまとめる
        egui::TopBottomPanel::top("menu").exact_height(28.0).frame(bar_frame(theme::BG0, 4.0)).show(ctx, |ui| self.menu_bar(ctx, ui));
        let height = ctx.screen_rect().height();
        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height((height * 0.36).max(240.0))
            .height_range(140.0..=(height - 220.0).max(140.0))
            .frame(theme::panel_frame())
            .show(ctx, |ui| self.timeline(ctx, ui));
        egui::SidePanel::left("project")
            .resizable(true)
            .default_width(344.0)
            .width_range(220.0..=560.0)
            .frame(theme::panel_frame())
            .show(ctx, |ui| self.project_tab(ui));
        egui::SidePanel::right("properties")
            .resizable(true)
            .default_width(336.0)
            .width_range(270.0..=600.0)
            .frame(theme::panel_frame())
            .show(ctx, |ui| self.right_panel(ui));
        egui::CentralPanel::default().frame(theme::panel_frame()).show(ctx, |ui| self.comp_panel(ctx, ui));
    }

    // ---------- ツール（メニューの右に並ぶ） ----------

    /// ツールの切り替え・レイヤー作成・書き出し・プラグインのボタン。
    /// 補助表示の切り替えはビューアの下、スナップはタイムラインの上にあるので、ここには置かない。
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 1.0;
        for (tool, icon, tip) in Tool::ALL {
            if widgets::icon_toggle(ui, icon, self.tool == tool, tip).clicked() {
                self.set_tool(tool);
            }
        }
        widgets::vsep(ui);
        self.new_layer_button(ui);
        if widgets::icon_button(ui, Icon::Export, "書き出し…").clicked() {
            self.show.export = true;
        }
        self.plugin_toolbar(ui);
    }

    /// レイヤー作成のボタン。押すと、種類を絵と名前で選ぶ一覧が開く。
    fn new_layer_button(&mut self, ui: &mut egui::Ui) {
        let resp = widgets::icon_button(ui, Icon::Plus, "レイヤーを作成…");
        // 一覧が開くボタンであることを示す小さな三角
        let c = resp.rect.right_bottom() - vec2(4.0, 4.0);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![c + vec2(-3.0, 0.0), c, c + vec2(0.0, -3.0)],
            theme::TEXT_DIM,
            Stroke::NONE,
        ));
        let id = ui.make_persistent_id("new_layer_popup");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(190.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            for (icon, label, what) in [
                (Icon::Text, "テキスト", NewLayer::Text),
                (Icon::Rect, "シェイプ（矩形）", NewLayer::Rect),
                (Icon::Ellipse, "シェイプ（楕円）", NewLayer::Ellipse),
                (Icon::Star, "シェイプ（スター）", NewLayer::Star),
                (Icon::Solid, "平面", NewLayer::Solid),
                (Icon::Null, "ヌルオブジェクト", NewLayer::Null),
                (Icon::Adjust, "調整レイヤー", NewLayer::Adjustment),
                (Icon::Camera, "カメラ", NewLayer::Camera),
                (Icon::Light, "ライト（ポイント）", NewLayer::Light(LightType::Point)),
                (Icon::Light, "ライト（スポット）", NewLayer::Light(LightType::Spot)),
                (Icon::Light, "ライト（平行）", NewLayer::Light(LightType::Parallel)),
                (Icon::Light, "ライト（アンビエント）", NewLayer::Light(LightType::Ambient)),
            ] {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 1.0, theme::BG3);
                }
                icons::paint_sized(ui.painter(), Rect::from_min_size(rect.min + vec2(2.0, 2.0), vec2(16.0, 16.0)), icon, theme::TEXT_DIM, 13.0);
                ui.painter().text(pos2(rect.left() + 24.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(11.5), theme::TEXT);
                if resp.clicked() {
                    self.create_layer(what);
                }
            }
        });
    }

    // ---------- 状態表示（最上段の右端） ----------

    /// 右から: ファイル名・未保存の印・ビット深度・レンダリング先・音声の処理・メッセージ（または進捗）。
    pub(crate) fn status_area(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().override_font_id = Some(FontId::proportional(10.5));
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.add_space(4.0);
        let title = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未保存".into());
        ui.label(egui::RichText::new(title).color(theme::TEXT));
        if self.dirty {
            let (r, _) = ui.allocate_exact_size(vec2(6.0, 12.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 2.5, theme::WARN);
            r_tooltip(ui, r, "保存していない変更があります");
        }
        let depth = match self.shared.settings.bit_depth {
            BitDepth::B8 => "8 bpc",
            BitDepth::B16 => "16 bpc",
            BitDepth::F32 => "32 bpc",
        };
        ui.label(egui::RichText::new(depth).color(theme::TEXT_DIM));
        let gpu = match crate::gpu_name() {
            Some(name) if crate::GPU_ENABLED.load(std::sync::atomic::Ordering::Relaxed) => format!("GPU: {name}"),
            _ => "CPU レンダリング".to_string(),
        };
        ui.label(egui::RichText::new(gpu).color(theme::TEXT_DIM));
        if let Some(task) = &self.audio_task {
            ui.label(egui::RichText::new(format!("{}…", task.label)).color(theme::TEXT_DIM));
        }
        widgets::vsep(ui);
        match self.running_task() {
            Some((label, fraction, cancel)) => {
                if ui.small_button("中止").clicked() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                ui.add(egui::ProgressBar::new(fraction).desired_width(180.0).desired_height(12.0).text(label));
            }
            None => {
                let status = self.status.clone();
                ui.add(egui::Label::new(egui::RichText::new(&status).color(theme::TEXT_DIM)).truncate()).on_hover_text(status);
            }
        }
    }

    // ---------- 左: プロジェクト ----------

    fn project_items(&self, shared: &Project) -> Vec<Item> {
        let search = self.project_search.trim().to_string();
        let keep = |name: &str| widgets::matches(&search, name);
        let mut items = Vec::new();
        for c in shared.compositions.iter().filter(|c| keep(&c.name)) {
            items.push(Item {
                id: ItemId::Comp(c.id.clone()),
                icon: Icon::Comp,
                name: c.name.clone(),
                kind: "コンポジション",
                size: format!("{}×{}", c.width, c.height),
                fps: format!("{:.2}", c.frame_rate.fps()),
                duration: timecode(c.frame_rate, c.frame_count()),
                selected: c.id == self.comp_id && self.selected_footage.is_none(),
                warn: false,
                locked: c.locked_by.is_some(),
            });
        }
        for f in shared.footage.iter().filter(|f| keep(&f.name)) {
            let (icon, kind) = match (f.width, f.frame_rate) {
                (0, _) => (Icon::Audio, "オーディオ"),
                (_, None) => (Icon::Image, "静止画"),
                _ => (Icon::Footage, "ムービー"),
            };
            items.push(Item {
                id: ItemId::Footage(f.id.clone()),
                icon,
                name: f.name.clone(),
                kind,
                size: if f.width == 0 { String::new() } else { format!("{}×{}", f.width, f.height) },
                fps: f.frame_rate.map(|r| format!("{:.2}", r.fps())).unwrap_or_default(),
                duration: f.duration.or(f.audio.map(|a| a.duration)).map(|d| format!("{:.2} 秒", d.seconds())).unwrap_or_default(),
                selected: self.selected_footage.as_ref() == Some(&f.id),
                warn: !rights::issues(f).is_empty(),
                locked: false,
            });
        }
        items
    }

    fn project_tab(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let items = self.project_items(&shared);

        // 下段: 選択中の項目の設定
        egui::TopBottomPanel::bottom("project_detail")
            .resizable(true)
            .default_height(236.0)
            .height_range(60.0..=520.0)
            .frame(theme::panel_frame())
            .show_inside(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if let Some(item) = self.selected_footage.clone().and_then(|id| shared.footage_item(&id)) {
                        widgets::section(ui, "フッテージと権利情報", true, |ui| self.footage_panel(ui, item));
                    } else if let Some(comp) = shared.comp(&self.comp_id) {
                        widgets::section(ui, "コンポジション設定", true, |ui| self.comp_settings(ui, comp));
                    }
                });
            });
        // 上段: 検索と操作を 1 行に
        egui::Frame::none().inner_margin(egui::Margin::symmetric(4.0, 3.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let w = (ui.available_width() - 5.0 * 24.0 - 6.0).max(60.0);
                widgets::search_box(ui, &mut self.project_search, "名前で検索", w);
                ui.add_space(6.0);
                if widgets::icon_button(ui, Icon::Import, "素材を読み込む…").clicked() {
                    self.import_footage_dialog(false);
                }
                if widgets::icon_button(ui, Icon::Footage, "連番画像を読み込む…").clicked() {
                    self.import_footage_dialog(true);
                }
                if widgets::icon_button(ui, Icon::NewComp, "新規コンポジション").clicked() {
                    self.new_composition();
                }
                let can_add = self.selected_footage.is_some();
                if ui
                    .add_enabled_ui(can_add, |ui| widgets::icon_button(ui, Icon::Plus, "選択中の素材を、現在のコンポにレイヤーとして追加"))
                    .inner
                    .clicked()
                {
                    self.add_selected_footage();
                }
                if widgets::icon_button(ui, Icon::Trash, "選択中の項目を削除（使われていないものだけ）").clicked() {
                    self.delete_project_item();
                }
            });
        });

        // 一覧（名前・種類・サイズ・フレームレート・デュレーション）
        let width = ui.available_width();
        let cols: &[(&str, f32)] = if width >= 300.0 {
            &[("種類", 64.0), ("サイズ", 62.0), ("fps", 34.0), ("デュレーション", 62.0)]
        } else {
            &[("種類", 64.0), ("サイズ", 62.0)]
        };
        let name_w = (width - cols.iter().map(|c| c.1).sum::<f32>() - theme::SCROLL_W).max(60.0);
        let font = FontId::proportional(10.5);
        let (hrect, _) = ui.allocate_exact_size(vec2(width, 17.0), Sense::hover());
        let p = ui.painter_at(hrect);
        p.rect_filled(hrect, 0.0, theme::BG2);
        p.hline(hrect.x_range(), hrect.bottom() - 0.5, Stroke::new(1.0_f32, theme::LINE));
        p.text(
            pos2(hrect.left() + 24.0, hrect.center().y),
            Align2::LEFT_CENTER,
            format!("名前（{} 項目）", items.len()),
            FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
        let mut x = hrect.left() + name_w;
        for (label, w) in cols {
            p.vline(x, hrect.y_range(), Stroke::new(1.0_f32, theme::LINE));
            p.text(pos2(x + 5.0, hrect.center().y), Align2::LEFT_CENTER, *label, FontId::proportional(10.0), theme::TEXT_DIM);
            x += w;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if items.is_empty() {
                widgets::hint(ui, "該当する項目がありません");
            }
            for item in &items {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::click());
                let p = ui.painter_at(rect);
                if item.selected {
                    p.rect_filled(rect, 0.0, theme::ACCENT);
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, theme::BG3);
                }
                let text = if item.selected { Color32::WHITE } else { theme::TEXT };
                let sub = if item.selected { Color32::from_gray(0xd8) } else { theme::TEXT_DIM };
                icons::paint_sized(&p, Rect::from_min_size(rect.min + vec2(4.0, 1.0), vec2(16.0, 16.0)), item.icon, sub, 13.0);
                let mut name_end = rect.left() + name_w - 4.0;
                for (flag, icon, color) in [(item.warn, Icon::Warning, theme::WARN), (item.locked, Icon::Lock, sub)] {
                    if flag {
                        name_end -= 15.0;
                        icons::paint_sized(&p, Rect::from_min_size(pos2(name_end, rect.top() + 1.0), vec2(15.0, 16.0)), icon, color, 12.0);
                    }
                }
                p.with_clip_rect(Rect::from_min_max(pos2(rect.left() + 24.0, rect.top()), pos2(name_end, rect.bottom()))).text(
                    pos2(rect.left() + 24.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    &item.name,
                    FontId::proportional(11.5),
                    text,
                );
                let mut x = rect.left() + name_w;
                for ((_, w), value) in cols.iter().zip([item.kind, item.size.as_str(), item.fps.as_str(), item.duration.as_str()]) {
                    p.with_clip_rect(Rect::from_min_max(pos2(x, rect.top()), pos2(x + w - 2.0, rect.bottom()))).text(
                        pos2(x + 5.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        value,
                        font.clone(),
                        sub,
                    );
                    x += w;
                }
                let resp = if item.warn { resp.on_hover_text("権利が未確認です（下段で入力できます）") } else { resp };
                if resp.clicked() || resp.double_clicked() {
                    match &item.id {
                        ItemId::Comp(id) => {
                            self.selected_footage = None;
                            self.switch_comp(id.clone());
                        }
                        ItemId::Footage(id) => {
                            self.selected_footage = Some(id.clone());
                            self.selected = None;
                        }
                    }
                }
                // ダブルクリック: 素材は現在のコンポにレイヤーとして追加する
                if resp.double_clicked() && matches!(item.id, ItemId::Footage(_)) {
                    self.add_selected_footage();
                }
                resp.context_menu(|ui| {
                    if let ItemId::Comp(id) = &item.id {
                        if *id != self.comp_id && ui.button("現在のコンポにレイヤーとして追加").clicked() {
                            ui.close_menu();
                            self.add_precomp(id.clone(), &item.name);
                        }
                    } else if ui.button("現在のコンポにレイヤーとして追加").clicked() {
                        ui.close_menu();
                        self.add_selected_footage();
                    }
                });
            }
        });
    }

    fn add_selected_footage(&mut self) {
        let shared = self.shared.clone();
        let Some(item) = self.selected_footage.clone().and_then(|id| shared.footage_item(&id)) else { return };
        let id = item.id.clone();
        let kind = if item.width == 0 { LayerKind::Audio { footage: id } } else { LayerKind::Footage { footage: id } };
        self.add_layer("レイヤー追加", &item.name, kind);
    }

    fn add_precomp(&mut self, comp: CompId, name: &str) {
        self.add_layer("プリコンポ追加", name, LayerKind::Precomp { comp });
        if !self.project.validate().is_empty() {
            self.undo();
            self.redo.clear();
            self.status = "循環するため追加できません".into();
        }
    }

    /// 選択中の素材（無ければ現在のコンポ）を消す。どこかのレイヤーが使っている間は消さない。
    fn delete_project_item(&mut self) {
        let shared = self.shared.clone();
        let layers = || shared.compositions.iter().flat_map(|c| c.layers.iter());
        if let Some(id) = self.selected_footage.clone() {
            let used = layers().any(|l| {
                matches!(&l.kind, LayerKind::Footage { footage } | LayerKind::Audio { footage } | LayerKind::Model { footage, .. } if *footage == id)
            });
            if used {
                self.status = "この素材を使っているレイヤーがあるため削除できません".into();
                return;
            }
            self.edit("素材を削除", false, |p| p.footage.retain(|f| f.id != id));
            self.selected_footage = None;
            return;
        }
        let id = self.comp_id.clone();
        if shared.compositions.len() <= 1 {
            self.status = "最後のコンポジションは削除できません".into();
        } else if layers().any(|l| matches!(&l.kind, LayerKind::Precomp { comp } if *comp == id)) {
            self.status = "このコンポジションを使っているレイヤーがあるため削除できません".into();
        } else {
            let next = shared.compositions.iter().find(|c| c.id != id).map(|c| c.id.clone());
            self.edit("コンポジションを削除", false, |p| p.compositions.retain(|c| c.id != id));
            if let Some(next) = next {
                self.switch_comp(next);
            }
        }
    }

    // ---------- 右: プロパティ／エフェクト一覧／オーディオ ----------

    fn right_panel(&mut self, outer: &mut egui::Ui) {
        // 中身が少しでもはみ出すと、パネルがその幅を覚えて次のフレームでさらに広がり続ける（イージングのタブなど）。
        // 中身は決まった矩形の中に描き、パネルの幅を変えるのは境目のドラッグだけにする
        let rect = outer.available_rect_before_wrap();
        outer.allocate_rect(rect, egui::Sense::hover());
        let mut inner = outer.new_child(egui::UiBuilder::new().max_rect(rect).layout(*outer.layout()));
        inner.set_clip_rect(rect.intersect(outer.clip_rect()));
        let ui = &mut inner;
        ui.spacing_mut().item_spacing.y = 0.0;
        widgets::tab_strip(ui, &["プロパティ", "エフェクト", "イージング", "オーディオ"], &mut self.right_tab);
        egui::ScrollArea::vertical().id_salt("right_upper_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            match self.right_tab {
                0 => self.properties_tab(ui),
                1 => self.effects_presets_tab(ui),
                2 => self.easing_tab(ui),
                _ => self.audio_tab(ui),
            }
        });
    }

    /// 選択中のレイヤーのすべての設定（種類ごとの設定・文字・トランスフォーム・マスク・エフェクト・音声）。
    /// 先頭の 1 行に、整列のボタンとデュレーション（イン／アウトはポインタを載せると出る）を置く。
    fn properties_tab(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        match self.selected.clone().and_then(|id| comp.layer(&id)) {
            Some(layer) => {
                self.layer_header(ui, comp, layer);
                self.layer_panel(ui, comp, layer);
            }
            None => widgets::hint(
                ui,
                "タイムラインかビューアでレイヤーを選択すると、トランスフォーム・文字・マスク・エフェクト・音声をここで編集できます。",
            ),
        }
    }

    fn layer_header(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer) {
        let fr = comp.frame_rate;
        let (a, b) = (fr.time_to_frame(layer.in_point), fr.time_to_frame(layer.out_point));
        egui::Frame::none().fill(theme::BG2).inner_margin(egui::Margin::symmetric(4.0, 2.0)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_enabled_ui(!layer.locked, |ui| {
                    for (icon, tip, horizontal, mode) in [
                        (Icon::AlignLeft, "コンポの左に揃える", true, -1),
                        (Icon::AlignHCenter, "コンポの水平方向の中央に揃える", true, 0),
                        (Icon::AlignRight, "コンポの右に揃える", true, 1),
                        (Icon::AlignTop, "コンポの上に揃える", false, -1),
                        (Icon::AlignVCenter, "コンポの垂直方向の中央に揃える", false, 0),
                        (Icon::AlignBottom, "コンポの下に揃える", false, 1),
                    ] {
                        if widgets::icon_button(ui, icon, tip).clicked() {
                            self.align_layer(horizontal, mode);
                        }
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let mono = |s: String| egui::RichText::new(s).font(FontId::monospace(10.5));
                    ui.label(mono(timecode(fr, b - a)).color(theme::VALUE))
                        .on_hover_text(format!("デュレーション（イン {} – アウト {}）", timecode(fr, a), timecode(fr, b)));
                    ui.label(egui::RichText::new("Δ").size(10.5).color(theme::TEXT_DIM))
                        .on_hover_text(format!("イン {} – アウト {}", timecode(fr, a), timecode(fr, b)));
                });
            });
        });
    }

    fn effects_presets_tab(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none().inner_margin(egui::Margin::symmetric(4.0, 3.0)).show(ui, |ui| {
            let w = ui.available_width() - 22.0;
            widgets::search_box(ui, &mut self.fx_search, "エフェクトを検索", w);
        });
        let search = self.fx_search.trim().to_string();
        let effects = all_effects();
        let mut categories: Vec<&str> = Vec::new();
        for d in &effects {
            if !categories.contains(&d.category) {
                categories.push(d.category);
            }
        }
        let mut add = None;
        for cat in categories {
            let found: Vec<_> =
                effects.iter().filter(|d| d.category == cat && widgets::matches(&search, &format!("{} {}", d.label, d.category))).collect();
            if found.is_empty() {
                continue;
            }
            // 検索中は該当する分類を開いておく
            let body = |ui: &mut egui::Ui| {
                for def in &found {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 1.0, theme::BG3);
                    }
                    icons::paint_sized(ui.painter(), Rect::from_min_size(rect.min, vec2(16.0, 18.0)), Icon::Fx, theme::TEXT_DIM, 13.0);
                    ui.painter().text(
                        pos2(rect.left() + 20.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        def.label,
                        FontId::proportional(11.5),
                        theme::TEXT,
                    );
                    if resp.on_hover_text("ダブルクリックで、選択中のレイヤーに適用").double_clicked() {
                        add = Some(def.kind);
                    }
                }
            };
            if search.is_empty() {
                widgets::section(ui, cat, false, body);
            } else {
                widgets::section(ui, &format!("{cat}（{} 件）", found.len()), true, body);
            }
        }
        if let Some(kind) = add {
            self.add_effect(kind);
        }
    }

    /// 再生の操作（先頭・コマ戻し・再生／停止・コマ送り・末尾）。
    fn transport(&mut self, ui: &mut egui::Ui) {
        if widgets::icon_button(ui, Icon::First, "先頭へ (Home)").clicked() {
            self.set_frame(0, false);
        }
        if widgets::icon_button(ui, Icon::Prev, "1 フレーム戻る (←)").clicked() {
            self.set_frame(self.frame - 1, false);
        }
        if widgets::icon_toggle(
            ui,
            if self.playing.is_some() { Icon::Pause } else { Icon::Play },
            self.playing.is_some(),
            "再生／停止 (Space)",
        )
        .clicked()
        {
            self.toggle_play();
        }
        if widgets::icon_button(ui, Icon::Next, "1 フレーム進む (→)").clicked() {
            self.set_frame(self.frame + 1, false);
        }
        if widgets::icon_button(ui, Icon::Last, "末尾へ (End)").clicked() {
            self.set_frame(i64::MAX, false);
        }
    }

    /// 選択中のレイヤーを、コンポの端または中央に揃える。`mode` は -1 = 左／上、0 = 中央、1 = 右／下。
    fn align_layer(&mut self, horizontal: bool, mode: i32) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let Some(layer) = self.selected.as_ref().and_then(|id| comp.layer(id)) else { return };
        let t = comp.frame_rate.frame_to_time(self.frame);
        let m = world_matrix(comp, layer, t);
        // 寸法の無いレイヤー（シェイプ・テキストなど）はアンカーポイントの位置で揃える
        let points: Vec<(f64, f64)> = match shared.source_size(&layer.kind) {
            Some((w, h)) => [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].iter().map(|(x, y)| m.apply(*x, *y)).collect(),
            None => {
                let a = layer.transform.anchor.eval(t);
                vec![m.apply(a[0], a[1])]
            }
        };
        let axis = |p: &(f64, f64)| if horizontal { p.0 } else { p.1 };
        let lo = points.iter().map(axis).fold(f64::INFINITY, f64::min);
        let hi = points.iter().map(axis).fold(f64::NEG_INFINITY, f64::max);
        let size = if horizontal { comp.width } else { comp.height } as f64;
        let d = match mode {
            -1 => -lo,
            0 => size / 2.0 - (lo + hi) / 2.0,
            _ => size - hi,
        };
        let (id, delta) = (layer.id.clone(), if horizontal { [d, 0.0] } else { [0.0, d] });
        self.edit_comp("整列", false, |c| edit::move_layer(c, &id, t, delta));
    }

    fn audio_tab(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        // 現在のフレームのピーク（左右）
        let per_frame = (SAMPLE_RATE as f64 / comp.frame_rate.fps()).ceil() as usize;
        let start = znd_audio::time_to_sample(comp.frame_rate.frame_to_time(self.frame)).max(0) as usize;
        let mut peak = [0.0f32; 2];
        for f in self.audio_buf.chunks_exact(2).skip(start).take(per_frame) {
            peak = [peak[0].max(f[0].abs()), peak[1].max(f[1].abs())];
        }
        let db = |v: f32| if v > 0.0 { 20.0 * v.log10() } else { f32::NEG_INFINITY };
        widgets::padded(ui, |ui| {
            ui.horizontal_top(|ui| {
                // レベルメーター（-48dB〜0dB）
                let (rect, _) = ui.allocate_exact_size(vec2(46.0, 118.0), Sense::hover());
                let p = ui.painter_at(rect);
                for (i, v) in peak.iter().enumerate() {
                    let lane = Rect::from_min_size(rect.min + vec2(i as f32 * 12.0, 0.0), vec2(10.0, rect.height()));
                    p.rect_filled(lane, 0.0, theme::INPUT);
                    let level = ((db(*v) + 48.0) / 48.0).clamp(0.0, 1.0);
                    let fill = Rect::from_min_max(pos2(lane.left(), lane.bottom() - level * lane.height()), lane.max);
                    p.rect_filled(
                        fill,
                        0.0,
                        if db(*v) > -1.0 {
                            theme::ERROR
                        } else if db(*v) > -6.0 {
                            theme::WARN
                        } else {
                            theme::OK
                        },
                    );
                }
                for mark in [0.0f32, -6.0, -12.0, -24.0, -36.0, -48.0] {
                    let y = rect.bottom() - (mark + 48.0) / 48.0 * rect.height();
                    p.text(
                        pos2(rect.left() + 26.0, y.clamp(rect.top() + 5.0, rect.bottom() - 5.0)),
                        Align2::LEFT_CENTER,
                        format!("{mark:.0}"),
                        FontId::monospace(9.0),
                        theme::TEXT_DIM,
                    );
                }
                ui.vertical(|ui| {
                    let fmt = |v: f32| if db(v).is_finite() { format!("{:.1} dB", db(v)) } else { "-∞ dB".to_string() };
                    ui.label(egui::RichText::new(format!("L {}　R {}", fmt(peak[0]), fmt(peak[1]))).font(FontId::monospace(10.5)));
                    if !self.has_audio() {
                        ui.label(egui::RichText::new("このコンポには音声がありません").size(10.5).color(theme::TEXT_DIM));
                    }
                    if let Some(layer) =
                        self.selected.as_ref().and_then(|id| comp.layer(id)).filter(|l| znd_audio::layer_footage(&shared, l).is_some())
                    {
                        ui.add_space(2.0);
                        ui.label(&layer.name);
                        self.audio_controls(ui, layer);
                    }
                    self.sync_controls(ui);
                });
            });
        });
    }

    // ---------- 中央: コンポジション ----------

    fn comp_panel(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let shared = self.shared.clone();
        // コンポごとのタブ
        let labels: Vec<String> = shared
            .compositions
            .iter()
            .map(|c| if c.id == self.comp_id { format!("コンポジション: {}", c.name) } else { c.name.clone() })
            .collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let current = shared.compositions.iter().position(|c| c.id == self.comp_id).unwrap_or(0);
        let mut tab = current;
        widgets::tab_strip(ui, &refs, &mut tab);
        if tab != current {
            if let Some(c) = shared.compositions.get(tab) {
                self.selected_footage = None;
                self.switch_comp(c.id.clone());
            }
        }
        egui::TopBottomPanel::bottom("viewer_footer")
            .exact_height(25.0)
            .frame(bar_frame(theme::BG2, 4.0))
            .show_inside(ui, |ui| self.viewer_footer(ui));
        egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::INPUT)).show_inside(ui, |ui| self.viewer(ctx, ui));
    }

    /// ビューアの下の操作列: 拡大率・解像度・チャンネル・補助表示・スナップショット・再生・現在時刻。
    fn viewer_footer(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let total = comp.frame_count().max(1);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let zoom_label = if self.zoom <= 0.0 { "全体表示".to_string() } else { format!("{:.0}%", self.zoom * 100.0) };
            widgets::combo(ui, "zoom", 64.0, zoom_label, 6, |ui, pick| {
                pick.value(ui, &mut self.zoom, 0.0, "全体表示");
                for z in [0.25, 0.5, 1.0, 2.0, 4.0] {
                    pick.value(ui, &mut self.zoom, z, format!("{:.0}%", z * 100.0));
                }
            });
            widgets::combo(ui, "resolution", 48.0, resolution_label(self.downsample), 3, |ui, pick| {
                for ds in [1, 2, 4] {
                    pick.value(ui, &mut self.downsample, ds, resolution_label(ds));
                }
            });
            const CHANNELS: [&str; 5] = ["RGB", "赤", "緑", "青", "輝度"];
            let before = self.channel;
            widgets::combo(ui, "channel", 48.0, CHANNELS[self.channel as usize % 5], CHANNELS.len(), |ui, pick| {
                for (i, name) in CHANNELS.iter().enumerate() {
                    pick.value(ui, &mut self.channel, i as u8, *name);
                }
            });
            if before != self.channel {
                // 表示用の画像を作り直す
                self.shown = None;
            }
            widgets::vsep(ui);
            ui.spacing_mut().item_spacing.x = 0.0;
            if widgets::icon_toggle(ui, Icon::Guides, self.show_guides, "セーフマージン（アクションセーフ 90%・タイトルセーフ 80%）")
                .clicked()
            {
                self.show_guides = !self.show_guides;
            }
            if widgets::icon_toggle(ui, Icon::Grid, self.show_grid, "グリッド").clicked() {
                self.show_grid = !self.show_grid;
            }
            if widgets::icon_toggle(
                ui,
                Icon::Screen,
                self.screening,
                "上映プレビュー: 白飛び（赤）・黒つぶれ（青）・バンディングの恐れ（緑）を重ねて表示",
            )
            .clicked()
            {
                self.screening = !self.screening;
            }
            if widgets::icon_button(ui, Icon::Snapshot, "スナップショット: 表示中のフレームを画像として保存…").clicked()
            {
                self.save_frame_image();
            }
            widgets::vsep(ui);
            self.transport(ui);
            widgets::vsep(ui);
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(egui::RichText::new(timecode(comp.frame_rate, self.frame)).font(FontId::monospace(11.5)).color(theme::VALUE));
            ui.label(egui::RichText::new(format!("{} / {}", self.frame, total)).font(FontId::monospace(10.5)).color(theme::TEXT_DIM));
            if let Some(g) = &comp.beat_grid {
                let b = g.beat_at(comp.frame_rate.frame_to_time(self.frame));
                let per = g.beats_per_bar.max(1) as f64;
                ui.label(
                    egui::RichText::new(format!("{} 小節 {} 拍", (b / per).floor() as i64 + 1, b.rem_euclid(per).floor() as i64 + 1))
                        .font(FontId::monospace(10.5))
                        .color(theme::BEAT),
                );
            }
            // 右端から: ポインタの下の座標と色・RAM プレビュー・ワークエリア・レンダリング中の印
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let dim = |s: String| egui::RichText::new(s).font(FontId::monospace(10.5)).color(theme::TEXT_DIM);
                if let Some((pos, c)) = self.hover_info {
                    ui.label(dim(format!("R{:>3} G{:>3} B{:>3} A{:>3}", c[0], c[1], c[2], c[3])));
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 12.0), Sense::hover());
                    ui.painter().rect_filled(r, 1.0, Color32::from_rgb(c[0], c[1], c[2]));
                    ui.painter().rect_stroke(r, 1.0, Stroke::new(1.0_f32, theme::LINE_HI));
                    ui.label(dim(format!("X{:>7.1} Y{:>7.1}", pos[0], pos[1])));
                }
                let cached = self.engine.cached_frames(self.revision, total, self.downsample).len();
                ui.label(dim(format!("RAM {cached}/{total}"))).on_hover_text("RAM プレビュー済みのフレーム数");
                if let Some((a, b)) = self.work_area {
                    let r = ui
                        .add(egui::Label::new(dim(format!("範囲 {a}–{b} ×"))).sense(Sense::click()))
                        .on_hover_text("ワークエリア（クリックで解除してコンポ全体に戻す）");
                    if r.clicked() {
                        self.work_area = None;
                    }
                }
                if !self.engine.is_cached(self.revision, self.frame, self.downsample) {
                    ui.label(egui::RichText::new("レンダリング中…").size(10.5).color(theme::TEXT_DIM));
                }
            });
        });
    }

    /// 表示中のフレーム（プレビュー解像度）を PNG で保存する。
    pub(crate) fn save_frame_image(&mut self) {
        let Some(p) = self.engine.get(self.revision, self.frame, self.downsample) else {
            self.status = "このフレームはまだレンダリング中です".into();
            return;
        };
        let name = format!("frame_{:05}.png", self.frame);
        let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(name).save_file() else { return };
        self.status = match image::save_buffer(&path, &p.rgba, p.width as u32, p.height as u32, image::ExtendedColorType::Rgba8) {
            Ok(()) => format!("{} に保存しました", path.display()),
            Err(e) => format!("保存できません: {e}"),
        };
    }
}

/// 描いただけの領域にツールチップを付ける。
fn r_tooltip(ui: &egui::Ui, rect: Rect, text: &str) {
    ui.interact(rect, ui.id().with(("tip", text)), Sense::hover()).on_hover_text(text);
}
