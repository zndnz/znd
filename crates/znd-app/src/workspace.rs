//! 画面全体の配置。最上段の 1 本にメニュー・ツール・状態表示、中段に プロジェクト｜ビューア｜プロパティ、下段にタイムライン。
//!
//! 同じ操作や情報を二か所に置かない。コンポの切り替えはプロジェクトの一覧、現在時刻はタイムラインの左上だけに置く。
//! 選んだもの（レイヤー・素材・コンポ）の設定はすべて右のプロパティに出す。
//! ビューアの表示の設定（拡大率・解像度・チャンネル・補助表示）は、ビューアの下の 1 つのボタンにまとめている。

use crate::icons::{self, Icon};
use crate::panels::NewLayer;
use crate::timeline::timecode;
use crate::tools::Tool;
use crate::{resolution_label, theme, widgets, App};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, Sense, Stroke};
use znd_audio::SAMPLE_RATE;
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
    /// 種類・大きさ・長さを 1 行にまとめた説明（ポインタを載せると出す）。
    detail: String,
    /// 行の右端に薄く出す短い説明（大きさか長さ）。
    brief: String,
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
        egui::TopBottomPanel::top("menu").exact_height(40.0).frame(bar_frame(theme::BG0, 8.0)).show(ctx, |ui| self.menu_bar(ctx, ui));
        let height = ctx.screen_rect().height();
        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height((height * 0.36).max(260.0))
            .height_range(160.0..=(height - 240.0).max(160.0))
            .frame(theme::panel_frame())
            .show(ctx, |ui| self.timeline(ctx, ui));
        egui::SidePanel::left("project")
            .resizable(true)
            .default_width(260.0)
            .width_range(200.0..=520.0)
            .frame(theme::panel_frame())
            .show(ctx, |ui| self.project_tab(ui));
        egui::SidePanel::right("properties")
            .resizable(true)
            .default_width(380.0)
            .width_range(300.0..=640.0)
            .frame(theme::panel_frame())
            .show(ctx, |ui| self.right_panel(ui));
        egui::CentralPanel::default().frame(theme::panel_frame()).show(ctx, |ui| self.comp_panel(ctx, ui));
    }

    // ---------- ツール（メニューの右に並ぶ） ----------

    /// ツールの切り替え・レイヤー作成・書き出し・プラグインのボタン。
    /// 補助表示の切り替えはビューアの下、スナップはタイムラインの上にあるので、ここには置かない。
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 2.0;
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
            ui.set_min_width(220.0);
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
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, 4.0, theme::BG3);
                }
                icons::paint_sized(ui.painter(), Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(18.0, 18.0)), icon, theme::TEXT_DIM, 15.0);
                ui.painter().text(pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), theme::TEXT);
                if resp.clicked() {
                    self.create_layer(what);
                }
            }
        });
    }

    // ---------- 状態表示（最上段の右端） ----------

    /// 右から: ファイル名（ポインタを載せるとビット深度とレンダリング先）・未保存の印・メッセージ（または進捗）。
    pub(crate) fn status_area(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().override_font_id = Some(FontId::proportional(12.0));
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.add_space(4.0);
        let title = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未保存".into());
        let depth = match self.shared.settings.bit_depth {
            BitDepth::B8 => "8 bpc",
            BitDepth::B16 => "16 bpc",
            BitDepth::F32 => "32 bpc",
        };
        let gpu = match crate::gpu_name() {
            Some(name) if crate::GPU_ENABLED.load(std::sync::atomic::Ordering::Relaxed) => format!("GPU: {name}"),
            _ => "CPU レンダリング".to_string(),
        };
        ui.label(egui::RichText::new(title).color(theme::TEXT)).on_hover_text(format!("{depth}・{gpu}"));
        if self.dirty {
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, theme::WARN);
            r_tooltip(ui, r, "保存していない変更があります");
        }
        match self.running_task() {
            Some((label, fraction, cancel)) => {
                if ui.small_button("中止").clicked() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                ui.add(egui::ProgressBar::new(fraction).desired_width(200.0).desired_height(14.0).text(label));
            }
            None => {
                // 音声の処理中はその名前を、それ以外は最後のメッセージを出す
                let status = match &self.audio_task {
                    Some(task) => format!("{}…", task.label),
                    None => self.status.clone(),
                };
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
            let duration = timecode(c.frame_rate, c.frame_count());
            items.push(Item {
                id: ItemId::Comp(c.id.clone()),
                icon: Icon::Comp,
                name: c.name.clone(),
                detail: format!("コンポジション・{}×{}・{:.2} fps・{duration}", c.width, c.height, c.frame_rate.fps()),
                brief: format!("{}×{}", c.width, c.height),
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
            let size = if f.width == 0 { String::new() } else { format!("{}×{}", f.width, f.height) };
            let duration = f.duration.or(f.audio.map(|a| a.duration)).map(|d| format!("{:.2} 秒", d.seconds()));
            let mut detail = vec![kind.to_string()];
            detail.extend((!size.is_empty()).then(|| size.clone()));
            detail.extend(f.frame_rate.map(|r| format!("{:.2} fps", r.fps())));
            detail.extend(duration.clone());
            items.push(Item {
                id: ItemId::Footage(f.id.clone()),
                icon,
                name: f.name.clone(),
                detail: detail.join("・"),
                brief: if size.is_empty() { duration.unwrap_or_default() } else { size },
                selected: self.selected_footage.as_ref() == Some(&f.id),
                warn: !rights::issues(f).is_empty(),
                locked: false,
            });
        }
        items
    }

    /// プロジェクトの一覧だけを置く。選んだ項目の設定は右のプロパティに出る。
    fn project_tab(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let items = self.project_items(&shared);

        // 上段: 検索と、読み込み・新規作成をまとめた「＋」
        egui::Frame::none().inner_margin(egui::Margin { left: 10.0, right: 8.0, top: 8.0, bottom: 6.0 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = (ui.available_width() - 32.0 - 30.0).max(60.0);
                widgets::search_box(ui, &mut self.project_search, "検索", w);
                self.project_add_button(ui);
            });
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if items.is_empty() {
                widgets::hint(ui, "該当する項目がありません");
            }
            for item in &items {
                let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                let rect = row.shrink2(vec2(6.0, 1.0));
                let p = ui.painter_at(row);
                if item.selected {
                    p.rect_filled(rect, 5.0, theme::ACCENT);
                } else if resp.hovered() {
                    p.rect_filled(rect, 5.0, theme::BG3);
                }
                let text = if item.selected { Color32::WHITE } else { theme::TEXT };
                let sub = if item.selected { Color32::from_gray(0xd8) } else { theme::TEXT_DIM };
                icons::paint_sized(&p, Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 9.0), vec2(18.0, 18.0)), item.icon, sub, 15.0);
                let mut name_end = rect.right() - 8.0;
                for (flag, icon, color) in [(item.warn, Icon::Warning, theme::WARN), (item.locked, Icon::Lock, sub)] {
                    if flag {
                        name_end -= 18.0;
                        icons::paint_sized(&p, Rect::from_min_size(pos2(name_end, rect.center().y - 9.0), vec2(18.0, 18.0)), icon, color, 14.0);
                    }
                }
                // 右端に大きさか長さを薄く出す（狭いときは名前を優先して出さない）
                let brief = p.layout_no_wrap(item.brief.clone(), FontId::proportional(11.5), sub);
                let name_x = rect.left() + 32.0;
                if name_end - brief.size().x - name_x > 80.0 {
                    name_end -= brief.size().x;
                    p.galley(pos2(name_end, rect.center().y - brief.size().y / 2.0), brief, sub);
                    name_end -= 10.0;
                }
                p.with_clip_rect(Rect::from_min_max(pos2(name_x, rect.top()), pos2(name_end, rect.bottom()))).text(
                    pos2(name_x, rect.center().y),
                    Align2::LEFT_CENTER,
                    &item.name,
                    FontId::proportional(13.0),
                    text,
                );
                let tip = if item.warn { format!("{}\n権利が未確認です（右のプロパティで入力できます）", item.detail) } else { item.detail.clone() };
                let resp = resp.on_hover_text(tip);
                if resp.clicked() || resp.double_clicked() || resp.secondary_clicked() {
                    self.select_project_item(&item.id);
                }
                // ダブルクリック: 素材は現在のコンポにレイヤーとして追加する
                if resp.double_clicked() && matches!(item.id, ItemId::Footage(_)) {
                    self.add_selected_footage();
                }
                resp.context_menu(|ui| {
                    let can_add = match &item.id {
                        ItemId::Comp(id) => *id != self.comp_id,
                        ItemId::Footage(_) => true,
                    };
                    if ui.add_enabled(can_add, egui::Button::new("現在のコンポにレイヤーとして追加")).clicked() {
                        ui.close_menu();
                        match &item.id {
                            ItemId::Comp(id) => self.add_precomp(id.clone(), &item.name),
                            ItemId::Footage(_) => self.add_selected_footage(),
                        }
                    }
                    ui.separator();
                    if ui.button("削除（使われていないものだけ）").clicked() {
                        ui.close_menu();
                        self.select_project_item(&item.id);
                        self.delete_project_item();
                    }
                });
            }
        });
    }

    /// 一覧の項目を選ぶ。コンポは開いてその設定を、素材はその設定と権利情報を、右のプロパティに出す。
    fn select_project_item(&mut self, id: &ItemId) {
        match id {
            ItemId::Comp(id) => {
                self.selected_footage = None;
                self.selected = None;
                self.switch_comp(id.clone());
            }
            ItemId::Footage(id) => {
                self.selected_footage = Some(id.clone());
                self.selected = None;
            }
        }
    }

    /// 素材の読み込み・連番の読み込み・新規コンポをまとめたボタン。
    fn project_add_button(&mut self, ui: &mut egui::Ui) {
        let resp = widgets::icon_button(ui, Icon::Plus, "読み込む・新規作成…");
        let id = ui.make_persistent_id("project_add_popup");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(200.0);
            if ui.button("素材を読み込む…").clicked() {
                self.import_footage_dialog(false);
            }
            if ui.button("連番画像を読み込む…").clicked() {
                self.import_footage_dialog(true);
            }
            ui.separator();
            if ui.button("新規コンポジション").clicked() {
                self.new_composition();
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
        widgets::tab_strip(ui, &["プロパティ", "イージング", "オーディオ"], &mut self.right_tab);
        egui::ScrollArea::vertical().id_salt("right_upper_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            match self.right_tab {
                0 => self.properties_tab(ui),
                1 => self.easing_tab(ui),
                _ => self.audio_tab(ui),
            }
        });
    }

    /// 選んだものの設定。レイヤーなら そのすべて（種類ごとの設定・文字・トランスフォーム・マスク・エフェクト・音声）、
    /// 素材なら その設定と権利情報、どちらも選んでいなければ現在のコンポの設定。
    fn properties_tab(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        if let Some(layer) = self.selected.clone().and_then(|id| comp.layer(&id)) {
            self.layer_header(ui, comp, layer);
            self.layer_panel(ui, comp, layer);
        } else if let Some(item) = self.selected_footage.clone().and_then(|id| shared.footage_item(&id)) {
            widgets::section(ui, "素材と権利情報", true, |ui| self.footage_panel(ui, item));
        } else {
            widgets::section(ui, &format!("コンポジション: {}", comp.name), true, |ui| self.comp_settings(ui, comp));
            widgets::hint(ui, "レイヤーを選ぶと、そのトランスフォーム・文字・マスク・エフェクト・音声をここで編集できます。");
        }
    }

    fn layer_header(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer) {
        let fr = comp.frame_rate;
        let (a, b) = (fr.time_to_frame(layer.in_point), fr.time_to_frame(layer.out_point));
        egui::Frame::none().inner_margin(egui::Margin::symmetric(8.0, 6.0)).show(ui, |ui| {
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
                    let mono = |s: String| egui::RichText::new(s).font(FontId::monospace(12.0));
                    ui.label(mono(timecode(fr, b - a)).color(theme::VALUE))
                        .on_hover_text(format!("デュレーション（イン {} – アウト {}）", timecode(fr, a), timecode(fr, b)));
                    ui.label(egui::RichText::new("Δ").size(12.0).color(theme::TEXT_DIM))
                        .on_hover_text(format!("イン {} – アウト {}", timecode(fr, a), timecode(fr, b)));
                });
            });
        });
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
                let (rect, _) = ui.allocate_exact_size(vec2(52.0, 150.0), Sense::hover());
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
                        FontId::monospace(10.0),
                        theme::TEXT_DIM,
                    );
                }
                ui.vertical(|ui| {
                    let fmt = |v: f32| if db(v).is_finite() { format!("{:.1} dB", db(v)) } else { "-∞ dB".to_string() };
                    ui.label(egui::RichText::new(format!("L {}　R {}", fmt(peak[0]), fmt(peak[1]))).font(FontId::monospace(12.0)));
                    if !self.has_audio() {
                        ui.label(egui::RichText::new("このコンポには音声がありません").size(12.0).color(theme::TEXT_DIM));
                    }
                    // レイヤーごとのミュート・フェード・ピッチはプロパティにある
                    self.sync_controls(ui);
                });
            });
        });
    }

    // ---------- 中央: コンポジション ----------

    fn comp_panel(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        // コンポの切り替えはプロジェクトの一覧で行う（ここにタブは置かない）
        egui::TopBottomPanel::bottom("viewer_footer")
            .exact_height(40.0)
            .frame(bar_frame(theme::BG1, 8.0))
            .show_inside(ui, |ui| self.viewer_footer(ui));
        egui::CentralPanel::default().frame(egui::Frame::none().fill(theme::INPUT)).show_inside(ui, |ui| self.viewer(ctx, ui));
    }

    /// ビューアの下の操作列: 表示の設定（1 つのボタンにまとめる）・再生・小節と拍・ポインタの下の色。
    /// 現在時刻はタイムラインの左上にあるので、ここには置かない。
    fn viewer_footer(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let total = comp.frame_count().max(1);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            self.view_options_button(ui, total);
            widgets::vsep(ui);
            self.transport(ui);
            if let Some(g) = &comp.beat_grid {
                ui.add_space(10.0);
                let b = g.beat_at(comp.frame_rate.frame_to_time(self.frame));
                let per = g.beats_per_bar.max(1) as f64;
                ui.label(
                    egui::RichText::new(format!("{} 小節 {} 拍", (b / per).floor() as i64 + 1, b.rem_euclid(per).floor() as i64 + 1))
                        .font(FontId::monospace(12.0))
                        .color(theme::BEAT),
                );
            }
            // 右端から: ポインタの下の座標と色・ワークエリア・レンダリング中の印
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let dim = |s: String| egui::RichText::new(s).font(FontId::monospace(11.5)).color(theme::TEXT_DIM);
                if let Some((pos, c)) = self.hover_info {
                    ui.label(dim(format!("R{:>3} G{:>3} B{:>3} A{:>3}", c[0], c[1], c[2], c[3])));
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0_f32, theme::LINE_HI));
                    ui.label(dim(format!("X{:>7.1} Y{:>7.1}", pos[0], pos[1])));
                }
                if let Some((a, b)) = self.work_area {
                    let r = ui
                        .add(egui::Label::new(dim(format!("範囲 {a}–{b} ×"))).sense(Sense::click()))
                        .on_hover_text("ワークエリア（クリックで解除してコンポ全体に戻す）");
                    if r.clicked() {
                        self.work_area = None;
                    }
                }
                if !self.engine.is_cached(self.revision, self.frame, self.downsample) {
                    ui.label(egui::RichText::new("レンダリング中…").size(12.0).color(theme::TEXT_DIM));
                }
            });
        });
    }

    /// ビューアの表示の設定をまとめたボタン。ラベルに拡大率と解像度を出し、押すと
    /// 拡大率・解像度・チャンネル・補助表示（セーフマージン・グリッド・上映プレビュー）・スナップショットを選べる。
    fn view_options_button(&mut self, ui: &mut egui::Ui, total: i64) {
        const CHANNELS: [&str; 5] = ["RGB", "赤", "緑", "青", "輝度"];
        let zoom_label = |z: f32| if z <= 0.0 { "全体表示".to_string() } else { format!("{:.0}%", z * 100.0) };
        let mut label = format!("{}・{}", zoom_label(self.zoom), resolution_label(self.downsample));
        if self.channel != 0 {
            label += &format!("・{}", CHANNELS[self.channel as usize % 5]);
        }
        if self.screening {
            label += "・上映";
        }
        let resp = ui.add(egui::Button::new(label).frame(true)).on_hover_text("表示の設定");
        let id = ui.make_persistent_id("view_options_popup");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        let before = self.channel;
        egui::popup_above_or_below_widget(ui, id, &resp, egui::AboveOrBelow::Above, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(260.0);
            let heading = |ui: &mut egui::Ui, s: &str| {
                ui.label(egui::RichText::new(s).size(11.5).color(theme::TEXT_DIM));
            };
            heading(ui, "拡大率");
            ui.horizontal_wrapped(|ui| {
                for z in [0.0, 0.25, 0.5, 1.0, 2.0, 4.0] {
                    ui.selectable_value(&mut self.zoom, z, zoom_label(z));
                }
            });
            heading(ui, "解像度");
            ui.horizontal(|ui| {
                for ds in [1, 2, 4] {
                    ui.selectable_value(&mut self.downsample, ds, resolution_label(ds));
                }
            });
            heading(ui, "チャンネル");
            ui.horizontal(|ui| {
                for (i, name) in CHANNELS.iter().enumerate() {
                    ui.selectable_value(&mut self.channel, i as u8, *name);
                }
            });
            ui.separator();
            ui.checkbox(&mut self.show_guides, "セーフマージン").on_hover_text("アクションセーフ 90%・タイトルセーフ 80%");
            ui.checkbox(&mut self.show_grid, "グリッド");
            ui.checkbox(&mut self.screening, "上映プレビュー").on_hover_text("白飛び（赤）・黒つぶれ（青）・バンディングの恐れ（緑）を重ねて表示");
            ui.separator();
            if ui.button("スナップショットを保存…").on_hover_text("表示中のフレームを画像として保存").clicked() {
                ui.memory_mut(|m| m.close_popup());
                self.save_frame_image();
            }
            let cached = self.engine.cached_frames(self.revision, total, self.downsample).len();
            ui.label(egui::RichText::new(format!("RAM プレビュー済み {cached} / {total} フレーム")).size(11.5).color(theme::TEXT_DIM));
        });
        if before != self.channel {
            // 表示用の画像を作り直す
            self.shown = None;
        }
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
