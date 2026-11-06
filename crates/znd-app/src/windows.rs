//! フローティングウィンドウ: 書き出し・レンダーキュー、提出前チェック、履歴、版、レビュー、グラフエディター、スクリプト。

use crate::panels::{apply_edit, path_label, KeyKind, PropEdit};
use crate::timeline::KeySel;
use crate::{base_dir, unix_now, widgets, App, Task};
use eframe::egui::{self, pos2, Color32, Sense, Stroke};
use znd_core::keys;
use znd_core::visit::find_prop;
use znd_core::*;
use znd_media::{ExportContext, ExportSettings, Format, QueueJob};

#[derive(Default)]
pub(crate) struct Shown {
    pub export: bool,
    pub check: bool,
    pub history: bool,
    pub snapshots: bool,
    pub reviews: bool,
    pub graph: bool,
    pub script: bool,
    pub licenses: bool,
    pub shortcuts: bool,
    pub plugins: bool,
    pub beatfit: bool,
}

pub(crate) struct ExportUi {
    pub format: usize,
    pub crf: u8,
    pub kbps: u32,
    pub audio: bool,
    pub pad: f64,
    pub work_area: bool,
    pub transparent: bool,
    pub report: bool,
    /// 権利未確認の素材があっても書き出す、という確認。
    pub acknowledged: bool,
    pub speed_graph: bool,
    /// プラグインの書き出し形式のパラメータ（`パッケージID/ID` → パラメータ ID → 値）。
    pub plugin_values: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

impl Default for ExportUi {
    fn default() -> Self {
        ExportUi {
            format: 0,
            crf: 14,
            kbps: 12000,
            audio: true,
            pad: 0.0,
            work_area: false,
            transparent: false,
            report: true,
            acknowledged: false,
            speed_graph: false,
            plugin_values: Default::default(),
        }
    }
}

const FORMATS: [&str; 10] = [
    "H.264（mp4）",
    "上映提出用 H.264（高画質・レポート付き）",
    "H.265（mp4、10bit）",
    "H.265 2 パス（目標ビットレート）",
    "ProRes 422（mov）",
    "ProRes 4444（mov、アルファ付き）",
    "PNG 連番",
    "PNG 連番（16bit）",
    "EXR 連番（リニア float）",
    "GIF（1/2 解像度）",
];

impl ExportUi {
    /// `format` が組み込みの数を超えていれば、プラグインの書き出し形式。
    fn plugin_exporter(&self) -> Option<znd_render::plugins::hooks::Exporter> {
        self.format.checked_sub(FORMATS.len()).and_then(|i| znd_render::plugins::hooks::exporters().get(i).cloned())
    }

    fn settings(&self, work_area: Option<(i64, i64)>, frame: i64) -> ExportSettings {
        if let Some(ex) = self.plugin_exporter() {
            let values = self.plugin_values.get(&ex.key).cloned().unwrap_or_default();
            let mut s = ExportSettings::new(Format::from_exporter(&ex, &values));
            s.audio = self.audio || ex.audio_only;
            s.transparent = self.transparent;
            if ex.single_frame {
                // 静止画は今のフレーム
                s.range = Some((frame, frame + 1));
            } else {
                s.pad_head = self.pad;
                s.pad_tail = self.pad;
                if self.work_area {
                    s.range = work_area;
                }
            }
            return s;
        }
        let mut s = match self.format {
            0 => ExportSettings::new(Format::H264 { crf: self.crf, preset: "slow".into() }),
            1 => ExportSettings::screening(self.pad),
            2 => ExportSettings::new(Format::H265 { crf: self.crf.max(10), preset: "medium".into(), two_pass_kbps: None }),
            3 => ExportSettings::new(Format::H265 { crf: 18, preset: "medium".into(), two_pass_kbps: Some(self.kbps) }),
            4 => ExportSettings::new(Format::ProRes422),
            5 => ExportSettings::new(Format::ProRes4444),
            6 => ExportSettings::new(Format::PngSequence { sixteen_bit: false }),
            7 => ExportSettings::new(Format::PngSequence { sixteen_bit: true }),
            8 => ExportSettings::new(Format::ExrSequence),
            _ => ExportSettings { downsample: 2, ..ExportSettings::new(Format::Gif) },
        };
        s.audio = self.audio;
        s.pad_head = self.pad;
        s.pad_tail = self.pad;
        s.transparent = self.transparent;
        if self.work_area {
            s.range = work_area;
        }
        s
    }
}

impl App {
    /// バックグラウンド処理の完了を拾う。
    pub(crate) fn poll_tasks(&mut self, _ctx: &egui::Context) {
        if let Some(result) = self.export_task.as_ref().and_then(|t| t.take()) {
            self.export_task = None;
            self.status = match result {
                Ok(report) => {
                    znd_media::notify("znd", "書き出しが終わりました");
                    if report.warnings.is_empty() {
                        format!("書き出しました（{} フレーム）", report.frames)
                    } else {
                        format!("書き出しました（{} フレーム）— 注意: {}", report.frames, report.warnings.join(" / "))
                    }
                }
                Err(e) => format!("書き出し失敗: {e}"),
            };
        }
        if let Some(result) = self.check_task.as_ref().and_then(|t| t.take()) {
            self.check_task = None;
            match result {
                Ok(r) => {
                    let problems = r.lines.iter().filter(|l| !l.0).count();
                    self.status = if problems == 0 {
                        "提出前チェック: 問題は見つかりませんでした".into()
                    } else {
                        format!("提出前チェック: {problems} 件の指摘があります")
                    };
                    self.check = r;
                }
                Err(e) => self.status = format!("チェックに失敗しました: {e}"),
            }
        }
    }

    fn start_export(&mut self, ctx: &egui::Context, jobs: Vec<QueueJob>, write_report: bool) {
        let (project, footage, clips, dir) =
            (self.shared.clone(), self.engine.footage.clone(), self.engine.clips.clone(), base_dir(self.path.as_deref()));
        let label =
            if jobs.len() == 1 { "書き出し中".to_string() } else { format!("キューを書き出し中（{} 件）", jobs.len()) };
        self.export_task = Some(Task::spawn(&label, ctx, move |report| {
            let exprs = znd_script::ExprEngine::new();
            let ectx = ExportContext {
                project: &project,
                footage: &*footage,
                clips: Some(&*clips),
                resolver: Some(&exprs),
                base_dir: Some(dir),
                accel: crate::gpu(),
                composite: crate::gpu_composite(),
            };
            let mut last = None;
            let mut failures = Vec::new();
            for job in &jobs {
                match znd_media::export(&ectx, &job.comp, &job.path, &job.settings, |d, t| report(d, t)) {
                    Ok(r) => {
                        if write_report {
                            if let Some(comp) = project.comp(&job.comp) {
                                let _ = std::fs::write(job.path.with_extension("report.txt"), r.to_text(comp));
                            }
                        }
                        last = Some(r);
                    }
                    Err(znd_media::ExportError::Cancelled) => return Err("中断しました".into()),
                    Err(e) => failures.push(format!("{}: {e}", job.path.display())),
                }
            }
            match (last, failures.is_empty()) {
                (Some(r), true) => Ok(r),
                (_, false) => Err(failures.join(" / ")),
                (None, true) => Err("キューが空です".into()),
            }
        }));
    }

    pub(crate) fn windows(&mut self, ctx: &egui::Context) {
        self.recovery_window(ctx);
        self.plugin_crash_window(ctx);
        self.export_window(ctx);
        self.check_window(ctx);
        self.history_window(ctx);
        self.snapshot_window(ctx);
        self.review_window(ctx);
        self.graph_window(ctx);
        self.script_window(ctx);
        self.plugin_window(ctx);
        self.beatfit_window(ctx);
        self.info_windows(ctx);
    }

    fn recovery_window(&mut self, ctx: &egui::Context) {
        let Some(auto) = self.recovery.clone() else { return };
        egui::Window::new("自動保存からの復旧").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(
                "保存済みのファイルより新しい自動保存が見つかりました。\n前回、保存せずに終了した（または異常終了した）可能性があります。",
            );
            ui.monospace(auto.display().to_string());
            ui.horizontal(|ui| {
                if ui.button("自動保存を復元").clicked() {
                    match io::load(&auto) {
                        Ok(p) => {
                            self.edit("自動保存を復元", false, |project| *project = p);
                            if self.project.comp(&self.comp_id).is_none() {
                                self.comp_id = self.project.compositions.first().map(|c| c.id.clone()).unwrap_or_else(|| CompId::new(""));
                            }
                            self.status = "自動保存を復元しました（⌘Z で元に戻せます）".into();
                        }
                        Err(e) => self.status = format!("復元できません: {e}"),
                    }
                    self.recovery = None;
                }
                if ui.button("無視").clicked() {
                    // 未保存プロジェクトの退避分は、無視を選んだら次回また聞かないよう消す
                    if self.path.is_none() {
                        for old in io::list_autosaves(&self.autosave_base()) {
                            let _ = std::fs::remove_file(old);
                        }
                    }
                    self.recovery = None;
                }
            });
        });
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.export;
        let shared = self.shared.clone();
        egui::Window::new("書き出し").open(&mut open).default_width(460.0).show(ctx, |ui| {
            let Some(comp) = shared.comp(&self.comp_id) else { return };
            let e = &mut self.export_ui;
            let exporters = znd_render::plugins::hooks::exporters();
            if e.format >= FORMATS.len() + exporters.len() {
                e.format = 0;
            }
            let current = match e.format.checked_sub(FORMATS.len()) {
                Some(i) => format!("{}（プラグイン）", exporters[i].label),
                None => FORMATS[e.format].to_string(),
            };
            widgets::combo(ui, "format", 330.0, current, FORMATS.len() + exporters.len(), |ui, pick| {
                for (i, f) in FORMATS.iter().enumerate() {
                    pick.value(ui, &mut e.format, i, *f);
                }
                if !exporters.is_empty() {
                    pick.heading(ui, "プラグイン");
                    for (i, ex) in exporters.iter().enumerate() {
                        if pick.item_tip(ui, e.format == FORMATS.len() + i, &ex.label, &ex.description) {
                            e.format = FORMATS.len() + i;
                        }
                    }
                }
            });
            if let Some(ex) = e.plugin_exporter() {
                let values = e.plugin_values.entry(ex.key.clone()).or_default();
                egui::Grid::new("plugin_export_params").num_columns(2).show(ui, |ui| {
                    for p in &ex.params {
                        let v = values.entry(p.id.clone()).or_insert_with(|| p.default_value());
                        ui.label(&p.label);
                        match p.kind.as_str() {
                            "choice" => {
                                let choices = p.choices();
                                let shown = choices.iter().find(|c| c.1 == *v).map(|c| c.0.clone()).unwrap_or_else(|| v.clone());
                                widgets::combo(ui, ("pexp", &p.id), 0.0, shown, choices.len(), |ui, pick| {
                                    for (label, value) in &choices {
                                        if pick.item(ui, v == value, label) {
                                            *v = value.clone();
                                        }
                                    }
                                });
                            }
                            "bool" => {
                                let mut b = v == "1";
                                if ui.checkbox(&mut b, "").changed() {
                                    *v = (b as u8).to_string();
                                }
                            }
                            "text" | "args" => {
                                ui.text_edit_singleline(v);
                            }
                            _ => {
                                let mut x: f64 = v.parse().unwrap_or(0.0);
                                let lo = p.min.unwrap_or(f64::MIN);
                                let hi = p.max.unwrap_or(f64::MAX);
                                if ui.add(egui::DragValue::new(&mut x).range(lo..=hi)).changed() {
                                    *v = if x.fract() == 0.0 { format!("{}", x as i64) } else { format!("{x}") };
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
                if ex.single_frame {
                    ui.weak("今のフレームを 1 枚書き出します");
                }
            }
            ui.horizontal(|ui| {
                if matches!(e.format, 0 | 2) {
                    ui.add(egui::DragValue::new(&mut e.crf).range(0..=40).prefix("画質 CRF "));
                    ui.weak("（小さいほど高画質）");
                }
                if e.format == 3 {
                    ui.add(egui::DragValue::new(&mut e.kbps).range(500..=200_000).suffix(" kbps"));
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut e.audio, "音声を含める");
                ui.add(egui::DragValue::new(&mut e.pad).speed(0.1).range(0.0..=30.0).prefix("前後の黒味 ").suffix(" 秒"));
                if matches!(e.format, 5..=8) || e.plugin_exporter().is_some_and(|x| x.alpha) {
                    ui.checkbox(&mut e.transparent, "背景を透過");
                }
            });
            ui.horizontal(|ui| {
                ui.add_enabled(self.work_area.is_some(), egui::Checkbox::new(&mut e.work_area, "ワークエリアだけ"));
                ui.checkbox(&mut e.report, "レポートを書き出す").on_hover_text("解像度・フレーム数・ラウドネス・警告をテキストで残します");
            });

            // 権利の確認
            let warnings = rights::export_warnings(&shared, &comp.id);
            if !warnings.is_empty() {
                ui.separator();
                ui.colored_label(crate::theme::WARN, "権利が確認できていない素材があります:");
                for w in &warnings {
                    ui.label(format!("・{w}"));
                }
                ui.checkbox(&mut e.acknowledged, "確認中であることを承知のうえで書き出す");
            }
            let allowed = warnings.is_empty() || e.acknowledged;
            let settings = e.settings(self.work_area, self.frame);
            let report = e.report;
            ui.separator();
            ui.horizontal(|ui| {
                let ext = settings.format.extension();
                let pick = |name: &str| rfd::FileDialog::new().set_file_name(format!("{name}.{ext}")).save_file();
                if ui.add_enabled(allowed && self.export_task.is_none(), egui::Button::new("書き出す…")).clicked() {
                    if let Some(path) = pick(&comp.name) {
                        let job = QueueJob { comp: comp.id.clone(), path, settings: settings.clone() };
                        self.start_export(ctx, vec![job], report);
                    }
                }
                if ui.add_enabled(allowed, egui::Button::new("キューに追加…")).clicked() {
                    if let Some(path) = pick(&comp.name) {
                        self.queue.push(QueueJob { comp: comp.id.clone(), path, settings: settings.clone() });
                    }
                }
            });
            if !self.queue.is_empty() {
                ui.separator();
                ui.strong("レンダーキュー");
                let mut remove = None;
                for (i, job) in self.queue.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{}. {} → {}",
                            i + 1,
                            job.comp,
                            job.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                        ));
                        if crate::widgets::small_icon(ui, crate::icons::Icon::Close, false, "キューから外す").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.queue.remove(i);
                }
                if ui.add_enabled(self.export_task.is_none(), egui::Button::new("キューを実行（バックグラウンド）")).clicked()
                {
                    let jobs = std::mem::take(&mut self.queue);
                    self.start_export(ctx, jobs, report);
                }
            }
            ui.weak("書き出し中も編集を続けられます。終わると通知します。");
        });
        self.show.export = open;
    }

    fn check_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.check;
        egui::Window::new("提出前チェック").open(&mut open).default_width(520.0).show(ctx, |ui| {
            if ui.add_enabled(self.check_task.is_none(), egui::Button::new("チェックを実行")).clicked() {
                self.start_check(ctx);
            }
            if self.check_task.is_some() {
                ui.weak("解析中…（全フレームを低解像度でレンダリングします）");
            }
            ui.separator();
            for (ok, text) in &self.check.lines {
                ui.horizontal(|ui| {
                    let (icon, color) = if *ok {
                        (crate::icons::Icon::Diamond, crate::theme::OK)
                    } else {
                        (crate::icons::Icon::Warning, crate::theme::WARN)
                    };
                    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 16.0), egui::Sense::hover());
                    crate::icons::paint_sized(ui.painter(), r, icon, color, 12.0);
                    ui.colored_label(color, text);
                });
            }
            if !self.check.flashes.is_empty() {
                ui.weak("明滅の区間はタイムラインに赤で表示されます");
            }
            ui.separator();
            ui.weak("光過敏性の検査は制作中の目安です。正式な適合検査の代わりにはなりません。");
        });
        self.show.check = open;
    }

    fn history_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.history;
        egui::Window::new("履歴").open(&mut open).default_width(260.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                let mut jump = None;
                for (i, (label, _)) in self.undo.iter().enumerate() {
                    if ui.selectable_label(false, format!("{}. {label}", i + 1)).on_hover_text("この操作の直前まで戻る").clicked()
                    {
                        jump = Some(i);
                    }
                }
                ui.label(egui::RichText::new("現在の状態").strong());
                let mut redo_to = None;
                for (i, (label, _)) in self.redo.iter().rev().enumerate() {
                    if ui.selectable_label(false, egui::RichText::new(format!("{label}（やり直し可）")).weak()).clicked() {
                        redo_to = Some(i + 1);
                    }
                }
                if let Some(i) = jump {
                    self.jump_to_history(i);
                }
                if let Some(n) = redo_to {
                    for _ in 0..n {
                        self.redo();
                    }
                }
            });
        });
        self.show.history = open;
    }

    fn snapshot_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.snapshots;
        egui::Window::new("版（スナップショット）").open(&mut open).default_width(420.0).show(ctx, |ui| {
            let Some(path) = self.path.clone() else {
                ui.label("版を残すには、先にプロジェクトを保存してください");
                return;
            };
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.snapshot_name);
                if ui.button("この状態を版として保存").clicked() {
                    self.status = match io::save_snapshot(&self.project, &path, &self.snapshot_name) {
                        Ok(_) => format!("版「{}」を保存しました", self.snapshot_name),
                        Err(e) => format!("保存できません: {e}"),
                    };
                }
            });
            ui.horizontal(|ui| {
                for preset in ["1次提出版", "2次提出版", "最終版"] {
                    if ui.small_button(preset).clicked() {
                        self.snapshot_name = preset.into();
                    }
                }
            });
            ui.separator();
            for (name, secs, file) in io::list_snapshots(&path).into_iter().rev() {
                ui.horizontal(|ui| {
                    let ago = unix_now().saturating_sub(secs);
                    let when = if ago < 3600 {
                        format!("{} 分前", ago / 60)
                    } else if ago < 86400 {
                        format!("{} 時間前", ago / 3600)
                    } else {
                        format!("{} 日前", ago / 86400)
                    };
                    ui.label(format!("{name}（{when}）"));
                    if ui.small_button("現在と比較").on_hover_text("いまのフレームで、この版との違いを赤く表示します").clicked()
                    {
                        self.compare_with(ctx, &file, &name);
                    }
                    if ui.small_button("この版に戻す").clicked() {
                        match io::load(&file) {
                            Ok(p) => {
                                self.edit(&format!("版「{name}」に戻す"), false, |project| *project = p);
                                self.status = format!("版「{name}」に戻しました（⌘Z で取り消せます）");
                            }
                            Err(e) => self.status = format!("読み込めません: {e}"),
                        }
                    }
                });
            }
            if self.diff.is_some() && ui.button("比較表示を閉じる").clicked() {
                self.diff = None;
            }
        });
        self.show.snapshots = open;
    }

    fn compare_with(&mut self, ctx: &egui::Context, file: &std::path::Path, name: &str) {
        let t = self.frame_rate().frame_to_time(self.frame);
        let result = io::load(file).map_err(|e| e.to_string()).and_then(|old| {
            znd_media::review::version_diff(&old, &self.shared, &*self.engine.footage, &self.comp_id, t, 2).map_err(|e| e.to_string())
        });
        match result {
            Ok(d) => {
                let rgba = znd_render::to_rgba8(&d.heatmap, None);
                let tex = ctx.load_texture(
                    "diff",
                    egui::ColorImage::from_rgba_unmultiplied([d.heatmap.width, d.heatmap.height], &rgba),
                    egui::TextureOptions::LINEAR,
                );
                let label = match d.bbox {
                    Some(_) => format!("版「{name}」との違い: {} 画素（赤い部分）", d.changed_pixels),
                    None => format!("版「{name}」との違いはありません（このフレーム）"),
                };
                self.diff = Some((tex, label));
            }
            Err(e) => self.status = format!("比較できません: {e}"),
        }
    }

    fn review_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.reviews;
        let shared = self.shared.clone();
        egui::Window::new("レビューコメント").open(&mut open).default_width(420.0).show(ctx, |ui| {
            let fr = self.frame_rate();
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.review_text);
                if ui
                    .add_enabled(!self.review_text.trim().is_empty(), egui::Button::new(format!("フレーム {} に追加", self.frame)))
                    .clicked()
                {
                    let (comp, t, user, text) =
                        (self.comp_id.clone(), fr.frame_to_time(self.frame), self.user.clone(), std::mem::take(&mut self.review_text));
                    let created = format!("{}", unix_now());
                    // コメントはロック中のコンポにも付けられる（レビューする側は編集者とは別の人）
                    self.edit("レビューコメント", false, |p| collab::add_review(p, &comp, t, &user, &text, None, &created));
                }
            });
            ui.separator();
            let mut resolve = None;
            let comp_id = self.comp_id.clone();
            for (i, r) in shared.reviews.iter().enumerate().filter(|(_, r)| r.comp == comp_id) {
                ui.horizontal(|ui| {
                    let frame = fr.time_to_frame(r.time);
                    if ui.small_button(format!("{frame}")).on_hover_text("このフレームへ移動").clicked() {
                        self.set_frame(frame, false);
                    }
                    let text = format!("{}: {}", r.author, r.text);
                    if r.resolved {
                        ui.weak(format!("（解決済み）{text}"));
                    } else {
                        ui.label(text);
                        if ui.small_button("解決").clicked() {
                            resolve = Some(i);
                        }
                    }
                });
            }
            if let Some(i) = resolve {
                self.edit("コメント解決", false, |p| p.reviews[i].resolved = true);
            }
        });
        self.show.reviews = open;
    }

    fn graph_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.graph;
        let shared = self.shared.clone();
        egui::Window::new("グラフエディター").open(&mut open).default_size([640.0, 320.0]).show(ctx, |ui| {
            let (Some(comp), Some(path)) = (shared.comp(&self.comp_id), self.graph_path.clone()) else {
                ui.label("プロパティのグラフのアイコンを押すと、ここにグラフが表示されます");
                return;
            };
            let Some(layer) = self.selected.as_ref().and_then(|id| comp.layer(id)) else {
                ui.label("レイヤーを選択してください");
                return;
            };
            let Some(prop) = find_prop(layer, &path) else {
                ui.label("このレイヤーには、そのプロパティがありません");
                return;
            };
            let fr = comp.frame_rate;
            let total = comp.frame_count().max(1);
            let t_now = fr.frame_to_time(self.frame);
            let id = layer.id.clone();
            let mut edit: Option<PropEdit> = None;
            ui.horizontal(|ui| {
                ui.strong(format!("{} › {}", layer.name, path_label(layer, &path)));
                ui.selectable_value(&mut self.export_ui.speed_graph, false, "値グラフ");
                ui.selectable_value(&mut self.export_ui.speed_graph, true, "速度グラフ");
            });
            // 操作の対象: このプロパティで選択中のキー。無ければ再生ヘッドの位置のキー
            let mut targets: Vec<KeySel> =
                self.selected_keys.iter().filter(|k| k.layer == id && k.path == path).cloned().collect();
            if targets.is_empty() && prop.key_times().contains(&t_now) {
                targets.push(KeySel { layer: id.clone(), path: path.clone(), at: t_now });
            }
            let mut style: Option<(&str, keys::KeyStyle)> = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("対象のキー {} 個:", targets.len()));
                for k in KeyKind::ALL {
                    if ui.small_button(k.label()).clicked() {
                        style = Some(("キーフレーム補間", k.style()));
                    }
                }
                ui.menu_button("イージングプリセット", |ui| {
                    for p in keys::EASE_PRESETS {
                        if ui.button(p.name).clicked() {
                            style = Some(("イージング", p.style));
                            ui.close_menu();
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                // ハンドルの数値入力（速度と影響度）
                let id_in = ui.id().with("ease-in");
                let id_out = ui.id().with("ease-out");
                let mut e_in: (f64, f64) = ui.data_mut(|d| *d.get_temp_mut_or(id_in, (0.0, 33.3)));
                let mut e_out: (f64, f64) = ui.data_mut(|d| *d.get_temp_mut_or(id_out, (0.0, 33.3)));
                let ease = |e: (f64, f64)| Ease { speed: e.0, influence: (e.1 / 100.0).clamp(0.001, 1.0) };
                ui.label("入る側");
                ui.add(egui::DragValue::new(&mut e_in.0).prefix("速度 "));
                ui.add(egui::DragValue::new(&mut e_in.1).range(0.1..=100.0).prefix("影響 ").suffix("%"));
                if ui.small_button("適用").clicked() {
                    style = Some((
                        "イーズ",
                        keys::KeyStyle {
                            interp_in: Some(Interp::Bezier),
                            bezier_mode: Some(BezierMode::Manual),
                            ease_in: Some(ease(e_in)),
                            ..keys::KeyStyle::new()
                        },
                    ));
                }
                ui.label("出る側");
                ui.add(egui::DragValue::new(&mut e_out.0).prefix("速度 "));
                ui.add(egui::DragValue::new(&mut e_out.1).range(0.1..=100.0).prefix("影響 ").suffix("%"));
                if ui.small_button("適用").clicked() {
                    style = Some((
                        "イーズ",
                        keys::KeyStyle {
                            interp_out: Some(Interp::Bezier),
                            bezier_mode: Some(BezierMode::Manual),
                            ease_out: Some(ease(e_out)),
                            ..keys::KeyStyle::new()
                        },
                    ));
                }
                ui.data_mut(|d| {
                    d.insert_temp(id_in, e_in);
                    d.insert_temp(id_out, e_out);
                });
            });
            if let Some((label, st)) = style {
                if targets.is_empty() {
                    self.status = "キーフレームを選択するか、再生ヘッドをキーに合わせてください".into();
                } else {
                    self.style_keys(label, &targets, st);
                }
            }

            let (rect, resp) = ui.allocate_exact_size(ui.available_size().max(egui::vec2(200.0, 160.0)), Sense::click_and_drag());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 0.0, Color32::from_gray(28));
            // 画素列ごとに値（または速度）を求める
            let cols = rect.width().max(2.0) as usize;
            let dims = prop.dims().max(1);
            let time_at = |c: usize| Time::from_seconds(comp.duration.seconds() * c as f64 / (cols - 1) as f64);
            let h = 0.5 / fr.fps();
            let sample = |t: Time| -> Vec<f64> {
                if self.export_ui.speed_graph {
                    let (a, b) = (
                        prop.eval(t - Time::from_seconds(h)).unwrap_or_default(),
                        prop.eval(t + Time::from_seconds(h)).unwrap_or_default(),
                    );
                    a.iter().zip(&b).map(|(x, y)| (y - x) / (2.0 * h)).collect()
                } else {
                    prop.eval(t).unwrap_or_default()
                }
            };
            let series: Vec<Vec<f64>> = (0..cols).map(|c| sample(time_at(c))).collect();
            let (lo, hi) = series.iter().flatten().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
            let (lo, hi) = if hi - lo < 1e-6 { (lo - 1.0, hi + 1.0) } else { (lo - (hi - lo) * 0.1, hi + (hi - lo) * 0.1) };
            let y_of = |v: f64| rect.bottom() - ((v - lo) / (hi - lo)) as f32 * rect.height();
            let x_of = |t: Time| rect.left() + (t.seconds() / comp.duration.seconds().max(1e-9)) as f32 * rect.width();
            const COLORS: [Color32; 4] = [
                Color32::from_rgb(255, 110, 110),
                Color32::from_rgb(110, 220, 130),
                Color32::from_rgb(120, 160, 255),
                Color32::from_rgb(230, 230, 120),
            ];
            for d in 0..dims.min(4) {
                let pts: Vec<egui::Pos2> =
                    series.iter().enumerate().filter_map(|(c, v)| v.get(d).map(|v| pos2(rect.left() + c as f32, y_of(*v)))).collect();
                p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, COLORS[d])));
            }
            p.text(
                rect.left_top() + egui::vec2(4.0, 2.0),
                egui::Align2::LEFT_TOP,
                format!("{hi:.1}"),
                egui::FontId::monospace(10.0),
                Color32::GRAY,
            );
            p.text(
                rect.left_bottom() + egui::vec2(4.0, -12.0),
                egui::Align2::LEFT_TOP,
                format!("{lo:.1}"),
                egui::FontId::monospace(10.0),
                Color32::GRAY,
            );
            let px = rect.left() + self.frame as f32 / total as f32 * rect.width();
            p.line_segment([pos2(px, rect.top()), pos2(px, rect.bottom())], Stroke::new(1.0_f32, Color32::from_rgb(255, 90, 90)));

            // キー: クリックでその時刻へ、ドラッグで時刻（フレーム単位）と値を動かす
            let keys = prop.key_times();
            let key_values: Vec<Vec<f64>> = keys.iter().map(|t| prop.eval(*t).unwrap_or_default()).collect();
            for (kt, kv) in keys.iter().zip(&key_values) {
                for (d, v) in kv.iter().enumerate().take(4) {
                    if !self.export_ui.speed_graph {
                        p.circle_filled(pos2(x_of(*kt), y_of(*v)), 4.0, if *kt == t_now { Color32::WHITE } else { COLORS[d] });
                    }
                }
                p.line_segment(
                    [pos2(x_of(*kt), rect.bottom() - 6.0), pos2(x_of(*kt), rect.bottom())],
                    Stroke::new(2.0_f32, Color32::WHITE),
                );
            }
            let drag_id = ui.id().with("drag-key");
            if resp.drag_started() || resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    // 最も近いキーをつかむ
                    let nearest = keys
                        .iter()
                        .copied()
                        .min_by(|a, b| (x_of(*a) - pos.x).abs().total_cmp(&(x_of(*b) - pos.x).abs()))
                        .filter(|t| (x_of(*t) - pos.x).abs() < 10.0);
                    ui.data_mut(|d| d.insert_temp(drag_id, nearest));
                    if let Some(t) = nearest {
                        let k = KeySel { layer: id.clone(), path: path.clone(), at: t };
                        let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                        self.click_keys(vec![k], additive && resp.clicked());
                    }
                    match nearest {
                        Some(t) => self.set_frame(fr.time_to_frame(t), false),
                        None => self.set_frame(((pos.x - rect.left()) / rect.width() * total as f32) as i64, true),
                    }
                }
            }
            if resp.dragged() {
                let grabbed: Option<Time> = ui.data(|d| d.get_temp(drag_id)).flatten();
                if let (Some(from), Some(pos)) = (grabbed, resp.interact_pointer_pos()) {
                    let frame =
                        (((pos.x - rect.left()) / rect.width()) as f64 * total as f64).round().clamp(0.0, (total - 1) as f64) as i64;
                    let to = fr.frame_to_time(frame);
                    if to != from && !keys.contains(&to) {
                        edit = Some(PropEdit::MoveKey { from, to });
                        for k in self.selected_keys.iter_mut().filter(|k| k.layer == id && k.path == path && k.at == from) {
                            k.at = to;
                        }
                        ui.data_mut(|d| d.insert_temp(drag_id, Some(to)));
                        self.frame = frame;
                    } else if !self.export_ui.speed_graph && dims == 1 {
                        // 1 次元のプロパティは、上下ドラッグで値を変えられる
                        let v = lo + (rect.bottom() - pos.y) as f64 / rect.height() as f64 * (hi - lo);
                        self.frame = fr.time_to_frame(from);
                        edit = Some(PropEdit::Set(vec![v]));
                    }
                }
            }
            if let Some(e) = edit {
                let t = fr.frame_to_time(self.frame);
                let at = match &e {
                    PropEdit::MoveKey { .. } => t,
                    _ => t,
                };
                self.edit_layer("グラフ編集", e.coalesce(), &id, |l| apply_edit(l, &path, at, &e));
            }
        });
        self.show.graph = open;
    }

    fn script_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.script;
        egui::Window::new("スクリプト").open(&mut open).default_width(520.0).show(ctx, |ui| {
            ui.weak("JavaScript でプロジェクト全体を書き換えます。`project` がプロジェクトファイルと同じ構造のオブジェクトです。");
            ui.add(egui::TextEdit::multiline(&mut self.script_text).code_editor().desired_rows(10).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if ui.button("実行").clicked() {
                    let mut copy = self.project.clone();
                    match znd_script::run_script(&mut copy, &self.script_text) {
                        Ok(logs) => {
                            self.edit("スクリプト", false, |p| *p = copy);
                            if self.project.comp(&self.comp_id).is_none() {
                                self.comp_id = self.project.compositions.first().map(|c| c.id.clone()).unwrap_or_else(|| CompId::new(""));
                            }
                            self.status = if logs.is_empty() { "スクリプトを実行しました".into() } else { format!("スクリプト: {}", logs.join(" / ")) };
                        }
                        Err(e) => self.status = format!("スクリプトエラー: {e}"),
                    }
                }
                if ui.button("ファイルから読み込む…").clicked() {
                    if let Some(s) = rfd::FileDialog::new().add_filter("JavaScript", &["js"]).pick_file().and_then(|p| std::fs::read_to_string(p).ok()) {
                        self.script_text = s;
                    }
                }
            });
            ui.weak("使える補助関数: comp(名前) / layer(コンポ, 名前) / forEachLayer(fn) / key(秒, 値, 'ease') / setKeys(プロパティ, [..]) / flicks(秒) / log(..)");
        });
        self.show.script = open;
    }

    fn info_windows(&mut self, ctx: &egui::Context) {
        let mut open = self.show.licenses;
        egui::Window::new("使用 OSS とライセンス").open(&mut open).default_width(520.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for c in licenses::COMPONENTS {
                    ui.strong(c.name);
                    ui.label(format!("ライセンス: {}", c.license));
                    ui.weak(c.usage);
                    ui.add_space(6.0);
                }
                ui.separator();
                ui.label(licenses::NOTICE);
            });
        });
        self.show.licenses = open;
        let mut open = self.show.shortcuts;
        egui::Window::new("キーボードショートカット").open(&mut open).default_size([560.0, 560.0]).show(ctx, |ui| {
            self.keymap_editor(ui);
        });
        self.show.shortcuts = open;
    }

    /// ショートカットの一覧と割り当ての変更。
    fn keymap_editor(&mut self, ui: &mut egui::Ui) {
        use crate::keymap::{self, Keymap, Preset, ACTIONS};
        ui.horizontal(|ui| {
            ui.label("プリセット");
            let mut preset = self.keymap.preset;
            widgets::combo(ui, "km-preset", 0.0, preset.label(), Preset::ALL.len(), |ui, pick| {
                for p in Preset::ALL {
                    pick.value(ui, &mut preset, p, p.label());
                }
            });
            if preset != self.keymap.preset {
                // プリセットを切り替えたら、個別の変更は捨てる
                self.keymap = Keymap { preset, overrides: Default::default() };
                self.save_keymap();
            }
            if ui.add_enabled(!self.keymap.overrides.is_empty(), egui::Button::new("変更をすべて戻す")).clicked() {
                self.keymap.overrides.clear();
                self.save_keymap();
            }
        });
        let conflicts = self.keymap.conflicts();
        if !conflicts.is_empty() {
            for (c, actions) in &conflicts {
                let names: Vec<&str> = actions.iter().map(|a| keymap::info(*a).label).collect();
                ui.colored_label(crate::theme::WARN, format!("{} が重複: {}（修飾キーの多いもの → 一覧の上にあるものが優先）", c.display(), names.join("、")));
            }
        }
        if let Some(a) = self.keymap_capture {
            ui.colored_label(crate::theme::VALUE, format!("「{}」に割り当てるキーを押してください（Esc で取り消し）", keymap::info(a).label));
        }
        ui.separator();
        let conflicted: Vec<keymap::Chord> = conflicts.iter().map(|(c, _)| *c).collect();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            let mut group = "";
            egui::Grid::new("km-grid").num_columns(3).min_col_width(150.0).striped(true).show(ui, |ui| {
                for i in ACTIONS {
                    if i.group != group {
                        group = i.group;
                        ui.strong(group);
                        ui.end_row();
                    }
                    ui.label(i.label);
                    let chords = self.keymap.chords(i.action);
                    let mut remove = None;
                    ui.horizontal(|ui| {
                        for (n, c) in chords.iter().enumerate() {
                            let text = egui::RichText::new(c.display());
                            let text = if conflicted.contains(c) { text.color(crate::theme::WARN) } else { text };
                            if ui.small_button(text).on_hover_text("クリックで外す").clicked() {
                                remove = Some(n);
                            }
                        }
                        if chords.is_empty() {
                            ui.weak("なし");
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.small_button("＋").on_hover_text("キーを追加").clicked() {
                            self.keymap_capture = Some(i.action);
                        }
                        let changed = self.keymap.overrides.contains_key(&i.action);
                        if ui.add_enabled(changed, egui::Button::new("既定").small()).on_hover_text("プリセットの割り当てに戻す").clicked() {
                            self.keymap.overrides.remove(&i.action);
                            self.save_keymap();
                        }
                    });
                    ui.end_row();
                    if let Some(n) = remove {
                        let mut cs = chords.clone();
                        cs.remove(n);
                        self.keymap.set(i.action, cs);
                        self.save_keymap();
                    }
                }
            });
            ui.add_space(8.0);
            ui.strong("マウス操作");
            egui::Grid::new("mouse").num_columns(2).show(ui, |ui| {
                for (key, what) in [
                    ("Shift + ドラッグ", "スナップせずにスクラブ／移動"),
                    ("Enter / Esc", "ペン: 開いたパスで確定 / 取り消し"),
                    ("Option + クリック", "マスクの頂点: 角 ⇄ 滑らか"),
                    ("キーをクリック / Shift・⌘ + クリック", "キーフレームを選択 / 選択に足す・外す"),
                    ("空いたところからドラッグ", "矩形でキーフレームを選択（Shift・⌘ で追加）"),
                    ("選択キーをドラッグ", "まとめて移動（ビートグリッド等に吸着）"),
                    ("Alt + 選択の先頭／末尾のキーをドラッグ", "選択キーの時間を伸縮（反対の端が中心）"),
                    ("⌘C / ⌘V", "キーフレームをコピー／再生ヘッドの位置に貼り付け"),
                    ("レイヤーの行をドラッグ", "重ね順を並び替え（右クリックで複製・削除などのメニュー）"),
                    ("⌘ + ホイール", "タイムラインを拡大・縮小（ホイールだけで横スクロール）"),
                ] {
                    ui.monospace(key);
                    ui.label(what);
                    ui.end_row();
                }
            });
        });
        if let Some(p) = Keymap::path() {
            ui.weak(format!("設定ファイル: {}", p.display()));
        }
    }
}
