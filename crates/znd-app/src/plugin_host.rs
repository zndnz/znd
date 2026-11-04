//! プラグインパッケージの画面側: メニュー・ツールバー・レイヤーの右クリック・ショートカットへのコマンドの配置、
//! スクリプトで組み立てるパネル、出力フィルターの切り替え、安全性スキャンの結果の表示と自己責任での許可。

use crate::{theme, App};
use eframe::egui::{self, Color32, Key, KeyboardShortcut, Modifiers};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use znd_core::{CompId, LayerId};
use znd_render::plugins::{self, hooks, scan::RiskLevel, Command, Panel, SecurityReport};

/// 開いているパネルの状態。
#[derive(Default)]
pub(crate) struct PanelState {
    pub open: bool,
    /// スクリプトが持ち回る状態（`panel(state)` / `onEvent(..., state)` の state）。
    state: Value,
    items: Vec<Value>,
    /// 部品を作ったときの（リビジョン, 選択, コンポ）。変わったら作り直す。
    built_for: Option<(u64, Option<LayerId>, CompId)>,
    error: Option<String>,
}

#[derive(Default)]
pub(crate) struct PluginHost {
    pub panels: HashMap<String, PanelState>,
    pub alert: Option<String>,
    /// `host.copyText` で頼まれた、次の描画でクリップボードに入れる文字。
    pub copy: Option<String>,
    pub logs: Vec<String>,
    /// 自己責任での許可を確認中のもの。
    pub confirm: Option<(PathBuf, SecurityReport)>,
    /// 詳細を開いているスキャン結果。
    pub detail: Option<PathBuf>,
    /// `host.runCommand` で頼まれた、次のフレームで実行するコマンド。
    queued: Vec<String>,
    /// `host.openFile` で読んだファイルを渡す、次のフレームで呼ぶ関数。
    calls: Vec<(Origin, String, Vec<Value>)>,
}

/// スクリプトがどのパッケージのどのファイルから実行されたか（コールバックを同じスクリプトで呼ぶため）。
#[derive(Clone)]
pub(crate) struct Origin {
    pub label: String,
    pub plugin: String,
    pub dir: PathBuf,
    pub source: std::sync::Arc<String>,
}

/// ファイルを読み書きするスクリプトに許す上限。
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

fn has_permission(plugin: &str, perm: &str) -> bool {
    plugins::packages().iter().any(|p| p.manifest.id == plugin && p.manifest.permissions.iter().any(|x| x.eq_ignore_ascii_case(perm)))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub(crate) fn risk_color(level: RiskLevel) -> Color32 {
    match level {
        RiskLevel::Safe => theme::OK,
        RiskLevel::Low => Color32::from_rgb(0x9a, 0xc0, 0x7a),
        RiskLevel::Medium => theme::WARN,
        RiskLevel::High => Color32::from_rgb(0xe8, 0x8a, 0x4a),
        RiskLevel::Critical => theme::ERROR,
    }
}

fn risk_badge(ui: &mut egui::Ui, level: RiskLevel) -> egui::Response {
    let text = egui::RichText::new(format!(" 危険度: {} ", level.short())).size(10.5).color(Color32::BLACK).strong();
    ui.add(egui::Label::new(text.background_color(risk_color(level))))
}

/// テキストファイルの中身を文字列にする。UTF-8（BOM 付き含む）と UTF-16 を見分け、
/// どちらでもなければ Shift_JIS として読む（AviUtl 由来の字幕などに多い）。
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |le: bool, b: &[u8]| -> String {
        let units: Vec<u16> = b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return utf16(true, rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return utf16(false, rest);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

/// "Cmd+Shift+K" のような表記を読む。
pub(crate) fn parse_shortcut(s: &str) -> Option<KeyboardShortcut> {
    let mut mods = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "mod" => mods = mods | Modifiers::COMMAND,
            "ctrl" | "control" => mods = mods | Modifiers::CTRL,
            "alt" | "option" | "opt" => mods = mods | Modifiers::ALT,
            "shift" => mods = mods | Modifiers::SHIFT,
            other => key = Key::from_name(other).or_else(|| Key::from_name(&other.to_ascii_uppercase())).or_else(|| Key::from_name(part)),
        }
    }
    Some(KeyboardShortcut::new(mods, key?))
}

impl App {
    /// スクリプトに渡す、画面の状態。
    fn plugin_context(&self, plugin_dir: &std::path::Path, plugin: &str) -> Value {
        let fps = self.frame_rate().fps();
        let t = self.frame_rate().frame_to_time(self.frame).seconds();
        json!({
            "comp": self.comp_id.0,
            "layer": self.selected.as_ref().map(|l| l.0.clone()),
            "layers": self.selected.iter().map(|l| l.0.clone()).collect::<Vec<_>>(),
            "frame": self.frame,
            "time": t,
            "fps": fps,
            "plugin": { "id": plugin, "dir": plugin_dir.display().to_string() },
            "projectPath": self.path.as_ref().map(|p| p.display().to_string()),
        })
    }

    /// プラグインのスクリプトの結果を取り込む（プロジェクトの書き換え・画面への操作・ログ）。
    fn absorb_plugin_run(&mut self, origin: &Origin, coalesce: bool, run: znd_script::PluginRun) {
        let label = origin.label.as_str();
        if let Some(updated) = run.project {
            if let Some(owner) = self.shared.comp(&self.comp_id).and_then(|c| c.locked_by.clone()).filter(|o| o != &self.user) {
                self.status = format!("このコンポは {owner} がロックしているため、プラグインの変更を取り込みませんでした");
            } else {
                self.edit(&format!("プラグイン: {label}"), coalesce, |p| *p = updated);
            }
        }
        if !run.logs.is_empty() {
            self.plugins.logs.extend(run.logs.into_iter().map(|l| format!("[{label}] {l}")));
            let n = self.plugins.logs.len();
            if n > 200 {
                self.plugins.logs.drain(..n - 200);
            }
        }
        for a in run.actions {
            match a.get("type").and_then(Value::as_str).unwrap_or("") {
                "select" => self.selected = a.get("layer").and_then(Value::as_str).map(LayerId::new),
                "frame" => self.set_frame(a.get("frame").and_then(Value::as_i64).unwrap_or(0), false),
                "work_area" => {
                    self.work_area = match (a.get("start").and_then(Value::as_i64), a.get("end").and_then(Value::as_i64)) {
                        (Some(s), Some(e)) => {
                            let (s, e) = (s.clamp(0, self.total_frames()), e.clamp(0, self.total_frames()));
                            (e > s).then_some((s, e))
                        }
                        _ => None,
                    };
                }
                "time" => {
                    let fps = self.frame_rate().fps();
                    self.set_frame((a.get("seconds").and_then(Value::as_f64).unwrap_or(0.0) * fps).round() as i64, false);
                }
                "status" => self.status = a.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                "alert" => self.plugins.alert = a.get("text").and_then(Value::as_str).map(String::from),
                "copy_text" => self.plugins.copy = a.get("text").and_then(Value::as_str).map(String::from),
                "open_project_folder" => match self.path.as_ref().and_then(|p| p.parent()) {
                    Some(dir) => {
                        if let Err(e) = open_in_file_manager(dir) {
                            self.status = format!("{} を開けません: {e}", dir.display());
                        }
                    }
                    None => self.status = "プロジェクトがまだ保存されていません".into(),
                },
                "comp" => {
                    let id = CompId::new(a.get("comp").and_then(Value::as_str).unwrap_or(""));
                    if self.shared.comp(&id).is_some() {
                        self.comp_id = id;
                    }
                }
                "panel" => {
                    let key = a.get("key").and_then(Value::as_str).unwrap_or("").to_string();
                    let open = a.get("open").and_then(Value::as_bool).unwrap_or(true);
                    self.plugins.panels.entry(key).or_default().open = open;
                }
                "filter" => {
                    hooks::set_filter_enabled(a.get("key").and_then(Value::as_str).unwrap_or(""), a.get("on").and_then(Value::as_bool).unwrap_or(false));
                    self.publish();
                }
                "command" => {
                    if let Some(k) = a.get("key").and_then(Value::as_str) {
                        if self.plugins.queued.len() < 8 {
                            self.plugins.queued.push(k.to_string());
                        }
                    }
                }
                kind @ ("open_file" | "save_file" | "import") => {
                    if !has_permission(&origin.plugin, "filesystem") {
                        let msg = format!("[{label}] ファイルを扱うには plugin.json の permissions に \"filesystem\" が必要です");
                        self.status = msg.clone();
                        self.plugins.logs.push(msg);
                        continue;
                    }
                    let exts = strings(a.get("extensions"));
                    let mut dialog = rfd::FileDialog::new();
                    if !exts.is_empty() {
                        dialog = dialog.add_filter(exts.join(", "), &exts);
                    }
                    match kind {
                        "open_file" => {
                            if let Some(t) = a.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                                dialog = dialog.set_title(t);
                            }
                            let Some(path) = dialog.pick_file() else { continue };
                            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                            if size > MAX_FILE_BYTES {
                                self.status = format!("[{label}] ファイルが大きすぎます（64MB まで）");
                                continue;
                            }
                            let content = match std::fs::read(&path) {
                                Ok(bytes) if a.get("binary").and_then(Value::as_bool).unwrap_or(false) => json!(bytes),
                                Ok(bytes) => json!(decode_text(&bytes)),
                                Err(e) => {
                                    self.status = format!("[{label}] 読めません: {e}");
                                    continue;
                                }
                            };
                            let callback = a.get("callback").and_then(Value::as_str).unwrap_or("").to_string();
                            let data = a.get("data").cloned().unwrap_or(Value::Null);
                            if !callback.is_empty() && self.plugins.calls.len() < 8 {
                                self.plugins.calls.push((origin.clone(), callback, vec![content, json!(path.display().to_string()), data]));
                            }
                        }
                        "save_file" => {
                            let name = a.get("name").and_then(Value::as_str).unwrap_or("untitled");
                            let Some(path) = dialog.set_file_name(name).save_file() else { continue };
                            let bytes: Vec<u8> = match a.get("content") {
                                Some(Value::String(t)) => t.as_bytes().to_vec(),
                                Some(Value::Array(v)) => v.iter().map(|b| b.as_u64().unwrap_or(0) as u8).collect(),
                                _ => Vec::new(),
                            };
                            self.status = match std::fs::write(&path, bytes) {
                                Ok(()) => format!("{} に保存しました", path.display()),
                                Err(e) => format!("[{label}] 保存できません: {e}"),
                            };
                        }
                        _ => {
                            let sequence = a.get("sequence").and_then(Value::as_bool).unwrap_or(false);
                            for path in dialog.pick_files().unwrap_or_default() {
                                self.import_path(&path, sequence);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// `host.openFile` のコールバックなど、スクリプトの関数を引数付きで呼ぶ。
    fn run_plugin_call(&mut self, origin: &Origin, function: &str, args: &[Value]) {
        let context = self.plugin_context(&origin.dir, &origin.plugin);
        match znd_script::run_plugin(&self.shared, &origin.source, Some((function, args)), &context) {
            Ok(run) => self.absorb_plugin_run(origin, false, run),
            Err(e) => {
                self.status = format!("{}: {e}", origin.label);
                self.plugins.logs.push(format!("[{}] エラー: {e}", origin.label));
            }
        }
    }

    pub(crate) fn run_plugin_command(&mut self, cmd: &Command) {
        let context = self.plugin_context(&cmd.dir, &cmd.plugin);
        let call = cmd.function.as_deref().map(|f| (f, &[][..]));
        let origin = Origin { label: cmd.label.clone(), plugin: cmd.plugin.clone(), dir: cmd.dir.clone(), source: cmd.source.clone() };
        match znd_script::run_plugin(&self.shared, &cmd.source, call, &context) {
            Ok(run) => {
                // スクリプトが host.status で出したものがあれば、そちらが上書きする
                self.status = format!("{} を実行しました", cmd.label);
                self.absorb_plugin_run(&origin, false, run);
            }
            Err(e) => {
                self.status = format!("{}: {e}", cmd.label);
                self.plugins.logs.push(format!("[{}] エラー: {e}", cmd.label));
            }
        }
    }

    fn run_command_key(&mut self, key: &str) {
        if let Some(cmd) = plugins::commands().into_iter().find(|c| c.key == key) {
            self.run_plugin_command(&cmd);
        } else {
            self.status = format!("コマンド {key} がありません");
        }
    }

    // ---------- メニュー・ツールバー ----------

    /// メニューバーの「プラグイン」。
    pub(crate) fn plugins_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(240.0);
        let commands: Vec<Command> = plugins::commands().into_iter().filter(|c| c.menu.iter().any(|m| m == "plugins")).collect();
        let mut by_plugin: Vec<(String, Vec<Command>)> = Vec::new();
        for c in commands {
            match by_plugin.iter_mut().find(|(p, _)| *p == c.plugin_name) {
                Some((_, list)) => list.push(c),
                None => by_plugin.push((c.plugin_name.clone(), vec![c])),
            }
        }
        if by_plugin.is_empty() {
            ui.weak("コマンドを持つプラグインはありません");
        }
        for (name, list) in by_plugin {
            ui.label(egui::RichText::new(name).size(10.0).color(theme::TEXT_DIM));
            for c in list {
                let mut b = egui::Button::new(&c.label);
                if let Some(s) = c.shortcut.as_deref().and_then(parse_shortcut) {
                    b = b.shortcut_text(ui.ctx().format_shortcut(&s));
                }
                let resp = ui.add(b);
                let resp = if c.description.is_empty() { resp } else { resp.on_hover_text(&c.description) };
                if resp.clicked() {
                    ui.close_menu();
                    self.run_plugin_command(&c);
                }
            }
        }
        let panels = plugins::panels();
        if !panels.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("パネル").size(10.0).color(theme::TEXT_DIM));
            for p in panels {
                let st = self.plugins.panels.entry(p.key.clone()).or_default();
                ui.checkbox(&mut st.open, &p.title);
            }
        }
        let filters = hooks::output_filters();
        if !filters.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("出力フィルター（コンポの最終画像に掛ける）").size(10.0).color(theme::TEXT_DIM));
            for f in filters {
                let mut on = f.enabled;
                if ui.checkbox(&mut on, &f.label).on_hover_text(f.stage.label()).changed() {
                    hooks::set_filter_enabled(&f.key, on);
                    self.publish();
                }
            }
        }
        ui.separator();
        if ui.button("プラグインの管理と安全性…").clicked() {
            ui.close_menu();
            self.show.plugins = true;
        }
        if !self.plugins.logs.is_empty() && ui.button("プラグインのログを消す").clicked() {
            self.plugins.logs.clear();
            ui.close_menu();
        }
    }

    /// レイヤーの右クリックメニューに出すコマンド。
    pub(crate) fn plugin_layer_menu(&mut self, ui: &mut egui::Ui, layer: &LayerId) {
        let list: Vec<Command> = plugins::commands().into_iter().filter(|c| c.menu.iter().any(|m| m == "layer")).collect();
        if list.is_empty() {
            return;
        }
        ui.separator();
        ui.menu_button("プラグイン", |ui| {
            for c in list {
                if ui.button(&c.label).clicked() {
                    ui.close_menu();
                    self.selected = Some(layer.clone());
                    self.run_plugin_command(&c);
                }
            }
        });
    }

    /// ツールバーに出すコマンド。
    pub(crate) fn plugin_toolbar(&mut self, ui: &mut egui::Ui) {
        let list: Vec<Command> = plugins::commands().into_iter().filter(|c| c.menu.iter().any(|m| m == "toolbar")).collect();
        if list.is_empty() {
            return;
        }
        crate::widgets::vsep(ui);
        for c in list {
            let text = egui::RichText::new(&c.label).size(10.5);
            let tip = format!("{}（プラグイン: {}）", if c.description.is_empty() { &c.label } else { &c.description }, c.plugin_name);
            if ui.add(egui::Button::new(text).small()).on_hover_text(tip).clicked() {
                self.run_plugin_command(&c);
            }
        }
    }

    // ---------- 毎フレーム ----------

    pub(crate) fn tick_plugins(&mut self, ctx: &egui::Context) {
        // ショートカット（文字入力中は奪わない。組み込みのショートカットが先に処理される）
        if !ctx.wants_keyboard_input() {
            let mut hit = None;
            for c in plugins::commands() {
                if let Some(s) = c.shortcut.as_deref().and_then(parse_shortcut) {
                    if ctx.input_mut(|i| i.consume_shortcut(&s)) {
                        hit = Some(c);
                    }
                }
            }
            if let Some(c) = hit {
                self.run_plugin_command(&c);
            }
        }
        for key in std::mem::take(&mut self.plugins.queued) {
            self.run_command_key(&key);
        }
        for (origin, function, args) in std::mem::take(&mut self.plugins.calls) {
            self.run_plugin_call(&origin, &function, &args);
        }
        for panel in plugins::panels() {
            if self.plugins.panels.get(&panel.key).is_some_and(|s| s.open) {
                self.plugin_panel(ctx, &panel);
            }
        }
        if let Some(text) = self.plugins.copy.take() {
            ctx.copy_text(text);
        }
        if let Some(text) = self.plugins.alert.clone() {
            let mut open = true;
            let mut close = false;
            egui::Window::new("プラグインからのお知らせ").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label(text);
                if ui.button("閉じる").clicked() {
                    close = true;
                }
            });
            if !open || close {
                self.plugins.alert = None;
            }
        }
        self.plugin_confirm_window(ctx);
    }

    /// スクリプトのパネル。部品はスクリプトの `panel(state)` が返し、操作は `onEvent(id, value, state)` へ送る。
    fn plugin_panel(&mut self, ctx: &egui::Context, panel: &Panel) {
        let key = (self.revision, self.selected.clone(), self.comp_id.clone());
        let needs_build = self.plugins.panels.get(&panel.key).is_none_or(|s| s.built_for.as_ref() != Some(&key));
        if needs_build {
            self.build_panel(panel);
        }
        let Some(st) = self.plugins.panels.get_mut(&panel.key) else { return };
        let mut open = st.open;
        let items = st.items.clone();
        let error = st.error.clone();
        let mut events: Vec<(String, Value)> = Vec::new();
        egui::Window::new(&panel.title)
            .id(egui::Id::new(("plugin-panel", &panel.key)))
            .open(&mut open)
            .default_width(panel.width)
            .resizable(true)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                if let Some(e) = &error {
                    ui.colored_label(theme::ERROR, e);
                }
                for item in &items {
                    panel_item(ui, item, &mut events);
                }
                ui.add_space(2.0);
                ui.label(egui::RichText::new(format!("プラグイン: {}", panel.plugin)).size(9.5).color(theme::TEXT_OFF));
            });
        if let Some(st) = self.plugins.panels.get_mut(&panel.key) {
            st.open = open;
        }
        for (id, value) in events {
            self.panel_event(panel, &id, value);
        }
    }

    fn build_panel(&mut self, panel: &Panel) {
        let context = self.plugin_context(&panel.dir, &panel.plugin);
        let key = (self.revision, self.selected.clone(), self.comp_id.clone());
        let state = self.plugins.panels.get(&panel.key).map(|s| s.state.clone()).filter(|s| s.is_object()).unwrap_or_else(|| json!({}));
        // panel(state) は state を書き換えてもよい（既定値の設定など）ので、戻り値と一緒に state も受け取る
        let wrapper = format!("{}\n;function __panel(state) {{ var items = panel(state); return {{ items: items, state: state }}; }}", panel.source);
        let result = znd_script::run_plugin(&self.shared, &wrapper, Some(("__panel", &[state])), &context);
        let st = self.plugins.panels.entry(panel.key.clone()).or_default();
        st.built_for = Some(key);
        match result {
            Ok(run) => {
                st.error = None;
                if let Some(s) = run.ret.get("state").filter(|s| s.is_object()) {
                    st.state = s.clone();
                }
                st.items = run.ret.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                // panel() は表示を作るだけ。プロジェクトの書き換えは onEvent でしか受け付けない
            }
            Err(e) => st.error = Some(e),
        }
    }

    fn panel_event(&mut self, panel: &Panel, id: &str, value: Value) {
        let context = self.plugin_context(&panel.dir, &panel.plugin);
        let state = self.plugins.panels.get(&panel.key).map(|s| s.state.clone()).filter(|s| s.is_object()).unwrap_or_else(|| json!({}));
        let wrapper = format!(
            "{}\n;function __event(id, value, state) {{ if (typeof onEvent === 'function') onEvent(id, value, state); return state; }}",
            panel.source
        );
        match znd_script::run_plugin(&self.shared, &wrapper, Some(("__event", &[json!(id), value, state])), &context) {
            Ok(run) => {
                if let Some(st) = self.plugins.panels.get_mut(&panel.key) {
                    if run.ret.is_object() {
                        st.state = run.ret.clone();
                    }
                    st.error = None;
                    st.built_for = None;
                }
                // スライダーのドラッグなど、続けて届く同じ部品の操作は 1 つの取り消し単位にまとめる
                let origin = Origin {
                    label: format!("{} / {id}", panel.title),
                    plugin: panel.plugin.clone(),
                    dir: panel.dir.clone(),
                    source: panel.source.clone(),
                };
                self.absorb_plugin_run(&origin, true, run);
            }
            Err(e) => {
                if let Some(st) = self.plugins.panels.get_mut(&panel.key) {
                    st.error = Some(e);
                }
            }
        }
    }

    // ---------- 管理ウィンドウ ----------

    /// プラグインの一覧・安全性スキャンの結果・読み込み・置き場所。
    pub(crate) fn plugin_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.plugins;
        egui::Window::new("プラグイン").open(&mut open).default_width(640.0).default_height(560.0).show(ctx, |ui| {
            egui::Frame::none().fill(Color32::from_rgb(0x3a, 0x2c, 0x1c)).inner_margin(6.0).show(ui, |ui| {
                ui.colored_label(theme::WARN, "⚠ プラグインの導入・使用は自己責任です");
                ui.label(egui::RichText::new(plugins::DISCLAIMER).size(10.5));
                ui.label(
                    egui::RichText::new("ネイティブのプラグイン（.dylib / .so / .dll / OpenFX）はこのアプリと同じ権限で動き、通信やファイル操作も行えます。スクリプト（JavaScript）はプロジェクトの読み書きのみで、通信・ファイル・別プログラムには触れられません。")
                        .size(10.5)
                        .color(theme::TEXT_DIM),
                );
            });
            ui.add_space(4.0);
            self.plugin_toolbar_row(ui);
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.plugin_lists(ui);
            });
        });
        self.show.plugins = open;
    }

    fn plugin_toolbar_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("プラグインを読み込む…")
                .on_hover_text("ライブラリ（.dylib / .so / .dll / .ofx）を選びます。この起動の間だけ使えます。常に使うにはプラグインフォルダに置いてください")
                .clicked()
            {
                let picked = rfd::FileDialog::new().add_filter("プラグイン", &["dylib", "so", "dll", "ofx", "bundle"]).pick_file();
                if let Some(path) = picked {
                    self.load_plugin_path(&path);
                }
            }
            if ui.button("フォルダを読み込む…").on_hover_text("plugin.json のあるパッケージ、または OpenFX の .ofx.bundle").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.load_plugin_path(&path);
                }
            }
            if ui.button("再走査").clicked() {
                let report = plugins::scan_default();
                self.plugin_report = plugins::load_errors();
                self.status = format!("プラグインを走査しました（新しく {} 個）", report.loaded.len());
                self.publish();
            }
            if let Some(dir) = plugins::user_plugin_dir() {
                if ui.button("プラグインフォルダを開く").clicked() {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(e) = open_in_file_manager(&dir) {
                        self.status = format!("{} を開けません: {e}", dir.display());
                    }
                }
            }
        });
    }

    fn load_plugin_path(&mut self, path: &std::path::Path) {
        self.status = match plugins::load_plugin_file(path) {
            Ok(infos) => {
                let r = plugins::report_for(path).map(|r| format!("・危険度 {}", r.level.short())).unwrap_or_default();
                format!("{} を読み込みました（エフェクト {} 個{r}）", path.display(), infos.len())
            }
            Err(e) => {
                if e.starts_with(plugins::BLOCKED_PREFIX) {
                    if let Some((p, r)) = plugins::blocked().into_iter().find(|(p, _)| path.canonicalize().ok().as_ref() == Some(p)) {
                        self.plugins.detail = Some(p);
                        let _ = r;
                    }
                }
                format!("プラグインを読み込めません: {e}")
            }
        };
        // 定義が増えたので、このプラグインを使っているレイヤーを描き直す
        self.publish();
    }

    /// 前回の起動でプラグインの処理中に異常終了していたら、そのプラグインを無効にするか尋ねる。
    pub(crate) fn plugin_crash_window(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.plugin_crash.clone() else { return };
        let label = plugins::loaded().into_iter().find(|i| i.kind == kind).map(|i| i.label).unwrap_or_else(|| kind.clone());
        egui::Window::new("プラグインによる異常終了").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(format!("前回、プラグイン「{label}」の処理中にアプリが異常終了しました。"));
            ui.weak("無効にすると、このエフェクトは元の画像をそのまま通します（プロジェクトの設定は残ります）。プラグインのウィンドウからいつでも戻せます。");
            ui.horizontal(|ui| {
                if ui.button("このプラグインを無効にする").clicked() {
                    match plugins::guard::set_disabled(&kind, true) {
                        Ok(()) => self.status = format!("「{label}」を無効にしました"),
                        Err(e) => self.status = format!("無効にできません: {e}"),
                    }
                    self.plugin_crash = None;
                    self.publish_refresh();
                }
                if ui.button("そのまま使う").clicked() {
                    self.plugin_crash = None;
                }
            });
        });
    }

    fn plugin_lists(&mut self, ui: &mut egui::Ui) {
        // 無効にしたもの・不具合で止めたもの
        let disabled = plugins::guard::disabled();
        let faults = plugins::guard::faults();
        if !disabled.is_empty() || !faults.is_empty() {
            ui.colored_label(theme::WARN, egui::RichText::new("使っていないプラグイン").strong());
            for kind in &disabled {
                ui.horizontal(|ui| {
                    ui.label(format!("無効: {kind}"));
                    if ui.small_button("有効に戻す").clicked() {
                        if let Err(e) = plugins::guard::set_disabled(kind, false) {
                            self.status = format!("戻せません: {e}");
                        }
                        self.publish_refresh();
                    }
                });
            }
            for (kind, why) in &faults {
                ui.horizontal(|ui| {
                    ui.label(format!("停止中: {kind}"));
                    if plugins::guard::is_quarantined(kind) && ui.small_button("もう一度使う").clicked() {
                        plugins::guard::release(kind);
                        self.publish_refresh();
                    }
                    if !plugins::guard::is_disabled(kind) && ui.small_button("無効にする").clicked() {
                        if let Err(e) = plugins::guard::set_disabled(kind, true) {
                            self.status = format!("無効にできません: {e}");
                        }
                    }
                });
                ui.weak(why);
            }
            ui.separator();
        }
        // 止めているもの
        let blocked = plugins::blocked();
        if !blocked.is_empty() {
            ui.colored_label(theme::ERROR, egui::RichText::new(format!("読み込みを止めているプラグイン（{}）", blocked.len())).strong());
            ui.weak("安全性スキャンで危険度が「高」以上と判定されたため、読み込んでいません。");
            for (path, report) in &blocked {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        risk_badge(ui, report.level);
                        ui.strong(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    });
                    ui.weak(path.display().to_string());
                    report_view(ui, report, true);
                    ui.horizontal(|ui| {
                        if ui.button(egui::RichText::new("自己責任で許可して読み込む…").color(theme::ERROR)).clicked() {
                            self.plugins.confirm = Some((path.clone(), report.clone()));
                        }
                        ui.weak("許可は中身のハッシュに結び付けて記録され、ファイルが変わると再び止まります");
                    });
                });
            }
            ui.add_space(6.0);
        }

        // パッケージ
        let packages = plugins::packages();
        let loaded_pkgs: Vec<_> = packages.iter().filter(|p| p.status != plugins::PackageStatus::Blocked).collect();
        if !loaded_pkgs.is_empty() {
            ui.strong("パッケージ（コマンド・パネル・出力フィルター・エクスプレッション）");
            for pkg in loaded_pkgs {
                let m = &pkg.manifest;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        risk_badge(ui, pkg.report.level);
                        ui.strong(&m.name);
                        ui.weak(format!("{} {}", m.id, m.version));
                    });
                    if !m.description.is_empty() {
                        ui.label(&m.description);
                    }
                    let or = |s: &str| if s.is_empty() { "記載なし".to_string() } else { s.to_string() };
                    ui.label(format!("提供元: {}　ライセンス: {}　宣言された権限: {}", or(&m.vendor), or(&m.license), if m.permissions.is_empty() { "なし".into() } else { m.permissions.join(", ") }));
                    ui.weak(format!(
                        "コマンド {}・パネル {}・出力フィルター {}・エクスプレッション用ライブラリ {}・エフェクト {}",
                        pkg.commands.len(),
                        pkg.panels.len(),
                        m.output_filters.len(),
                        m.expressions.len(),
                        pkg.effect_count
                    ));
                    if let plugins::PackageStatus::Failed(e) = &pkg.status {
                        ui.colored_label(theme::ERROR, format!("読み込めません: {e}"));
                    }
                    for w in &pkg.warnings {
                        ui.colored_label(theme::WARN, w);
                    }
                    ui.weak(pkg.dir.display().to_string());
                    let open = self.plugins.detail.as_ref() == Some(&pkg.dir);
                    if ui.small_button(if open { "スキャン結果を閉じる" } else { "スキャン結果を見る" }).clicked() {
                        self.plugins.detail = if open { None } else { Some(pkg.dir.clone()) };
                    }
                    if open {
                        report_view(ui, &pkg.report, false);
                        if pkg.report.level.blocks() && ui.small_button("許可を取り消す（次の起動から止める）").clicked() {
                            self.status = match plugins::revoke(&pkg.dir) {
                                Ok(()) => "許可を取り消しました".into(),
                                Err(e) => e,
                            };
                        }
                    }
                });
            }
            ui.add_space(6.0);
        }

        // エフェクト
        let list = plugins::loaded();
        ui.strong("エフェクト（znd ネイティブ / OpenFX）");
        if list.is_empty() {
            ui.weak("読み込まれているエフェクトはありません。サンプルは examples/plugins、作り方は docs/06-plugins.md にあります。");
        }
        for info in &list {
            let report = info.report();
            ui.horizontal(|ui| {
                if let Some(r) = &report {
                    risk_badge(ui, r.level);
                }
                ui.strong(&info.label);
                ui.weak(format!("[{} / {}]", info.api.label(), info.category));
            });
            let or = |s: &str| if s.is_empty() { "記載なし".to_string() } else { s.to_string() };
            ui.label(format!("提供元: {}　版: {}　ライセンス: {}", or(&info.vendor), or(&info.version), or(&info.license)));
            ui.weak(format!("{}（パラメータ {} 個）　{}", info.kind, info.param_count, info.path.display()));
            if let Some(r) = report.filter(|r| !r.findings.is_empty()) {
                let open = self.plugins.detail.as_ref() == Some(&info.path);
                if ui.small_button(if open { "スキャン結果を閉じる" } else { "スキャン結果を見る" }).clicked() {
                    self.plugins.detail = if open { None } else { Some(info.path.clone()) };
                }
                if open {
                    report_view(ui, &r, false);
                }
            }
            ui.add_space(6.0);
        }

        let missing = plugins::missing(&self.shared);
        if !missing.is_empty() {
            ui.separator();
            ui.colored_label(theme::ERROR, "このプロジェクトが使っているのに、入っていないプラグイン:");
            for (kind, layer) in missing {
                ui.label(format!("{layer}: {kind}（このエフェクトは無視されます）"));
            }
        }
        let errors: Vec<_> = self.plugin_report.iter().filter(|(_, e)| !e.starts_with(plugins::BLOCKED_PREFIX)).collect();
        if !errors.is_empty() {
            ui.separator();
            ui.colored_label(theme::ERROR, "読み込めなかったファイル:");
            for (path, e) in errors {
                ui.label(format!("{}: {e}", path.display()));
            }
        }
        if !self.plugins.logs.is_empty() {
            ui.separator();
            ui.label("スクリプトのログ:");
            for l in self.plugins.logs.iter().rev().take(40) {
                ui.weak(l);
            }
        }
        let messages = plugins::ofx::recent_messages();
        if !messages.is_empty() {
            ui.separator();
            ui.label("OpenFX プラグインからのメッセージ:");
            for m in messages.iter().rev().take(8) {
                ui.weak(m);
            }
        }
    }

    /// 自己責任での許可の最終確認。
    fn plugin_confirm_window(&mut self, ctx: &egui::Context) {
        let Some((path, report)) = self.plugins.confirm.clone() else { return };
        let mut open = true;
        let mut decided: Option<bool> = None;
        egui::Window::new("危険度の高いプラグインを許可しますか？")
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    risk_badge(ui, report.level);
                    ui.strong(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                });
                ui.label(report.summary());
                for f in report.findings.iter().take(4) {
                    ui.label(format!("・{}", f.title));
                }
                if !report.endpoints.is_empty() {
                    ui.label(format!("通信先の候補: {}", report.endpoints.iter().take(4).cloned().collect::<Vec<_>>().join(", ")));
                }
                ui.add_space(4.0);
                ui.colored_label(
                    theme::ERROR,
                    "許可すると、このプラグインはこのアプリと同じ権限で動きます。情報の持ち出しや、外部からのプログラムの取得・実行が行われる恐れがあります。",
                );
                ui.label(plugins::DISCLAIMER);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("内容を理解し、自己責任で許可する").color(theme::ERROR)).clicked() {
                        decided = Some(true);
                    }
                    if ui.button("やめる").clicked() {
                        decided = Some(false);
                    }
                });
            });
        if decided == Some(true) {
            self.status = match plugins::allow_and_load(&path) {
                Ok(infos) => format!("自己責任で許可して読み込みました: {}（エフェクト {} 個）", path.display(), infos.len()),
                Err(e) => format!("読み込めません: {e}"),
            };
            self.plugin_report = plugins::load_errors();
            self.publish();
        }
        if decided.is_some() || !open {
            self.plugins.confirm = None;
        }
    }
}

/// スキャン結果の詳細。
fn report_view(ui: &mut egui::Ui, report: &SecurityReport, compact: bool) {
    ui.label(egui::RichText::new(report.summary()).color(risk_color(report.level)));
    if report.findings.is_empty() {
        ui.weak(format!("{} 個のファイルを調べ、通信・個人情報の読み取り・プログラムの実行などの痕跡は見つかりませんでした。", report.files_scanned));
        return;
    }
    for f in report.findings.iter().take(if compact { 5 } else { 64 }) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("[{}] +{}", f.category.label(), f.score)).size(10.5).color(theme::TEXT_DIM));
            ui.label(&f.title);
        });
        if !f.evidence.is_empty() {
            ui.label(egui::RichText::new(format!("    根拠: {}", f.evidence.join(", "))).size(10.0).monospace().color(theme::TEXT_DIM));
        }
    }
    if !report.endpoints.is_empty() && !compact {
        ui.label(egui::RichText::new(format!("見つかった通信先: {}", report.endpoints.join(", "))).size(10.0).color(theme::TEXT_DIM));
    }
    if !compact {
        ui.label(egui::RichText::new(format!("SHA-256: {}", report.hash)).size(9.5).monospace().color(theme::TEXT_OFF));
    }
}

fn num(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// パネルの部品 1 つを描き、操作があれば (id, 値) を積む。
fn panel_item(ui: &mut egui::Ui, item: &Value, events: &mut Vec<(String, Value)>) {
    let id = text(item, "id").to_string();
    let label = text(item, "label");
    match text(item, "type") {
        "heading" => {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(text(item, "text")).strong());
        }
        "label" => {
            ui.label(text(item, "text"));
        }
        "hint" => {
            ui.weak(text(item, "text"));
        }
        "separator" => {
            ui.separator();
        }
        "button" => {
            if ui.button(label).clicked() {
                events.push((id, Value::Null));
            }
        }
        "slider" => {
            let mut v = num(item, "value", 0.0);
            let (lo, hi) = (num(item, "min", 0.0), num(item, "max", 1.0));
            let mut s = egui::Slider::new(&mut v, lo.min(hi)..=hi.max(lo)).text(label);
            if let Some(step) = item.get("step").and_then(Value::as_f64) {
                s = s.step_by(step);
            }
            if ui.add(s).changed() {
                events.push((id, json!(v)));
            }
        }
        "number" => {
            ui.horizontal(|ui| {
                ui.label(label);
                let mut v = num(item, "value", 0.0);
                let speed = num(item, "speed", 0.5);
                if ui.add(egui::DragValue::new(&mut v).speed(speed)).changed() {
                    events.push((id.clone(), json!(v)));
                }
            });
        }
        "checkbox" => {
            let mut v = item.get("value").and_then(Value::as_bool).unwrap_or(false);
            if ui.checkbox(&mut v, label).changed() {
                events.push((id, json!(v)));
            }
        }
        "text" => {
            ui.horizontal(|ui| {
                if !label.is_empty() {
                    ui.label(label);
                }
                let mem = egui::Id::new(("plugin-text", &id));
                let mut v = ui.memory(|m| m.data.get_temp::<String>(mem)).unwrap_or_else(|| text(item, "value").to_string());
                let resp = ui.add(egui::TextEdit::singleline(&mut v).desired_width(f32::INFINITY));
                // 1 文字ごとではなく、確定したとき（Enter・フォーカスが外れたとき）に送る
                if resp.lost_focus() {
                    ui.memory_mut(|m| m.data.remove::<String>(mem));
                    if v != text(item, "value") {
                        events.push((id.clone(), json!(v)));
                    }
                } else if resp.changed() {
                    ui.memory_mut(|m| m.data.insert_temp(mem, v));
                }
            });
        }
        "choice" => {
            let options: Vec<String> = item.get("options").and_then(Value::as_array).map(|a| a.iter().filter_map(|o| o.as_str().map(String::from)).collect()).unwrap_or_default();
            let cur = num(item, "value", 0.0).max(0.0) as usize;
            ui.horizontal(|ui| {
                ui.label(label);
                egui::ComboBox::from_id_salt(("plugin-choice", &id)).selected_text(options.get(cur).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                    for (i, o) in options.iter().enumerate() {
                        if ui.selectable_label(i == cur, o).clicked() && i != cur {
                            events.push((id.clone(), json!(i)));
                        }
                    }
                });
            });
        }
        "color" => {
            let c: Vec<f32> = item.get("value").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_f64().map(|v| v as f32)).collect()).unwrap_or_default();
            let mut rgba = [c.first().copied().unwrap_or(1.0), c.get(1).copied().unwrap_or(1.0), c.get(2).copied().unwrap_or(1.0), c.get(3).copied().unwrap_or(1.0)];
            ui.horizontal(|ui| {
                ui.label(label);
                if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                    events.push((id.clone(), json!(rgba)));
                }
            });
        }
        "row" => {
            ui.horizontal_wrapped(|ui| {
                for sub in item.get("items").and_then(Value::as_array).into_iter().flatten() {
                    panel_item(ui, sub, events);
                }
            });
        }
        other => {
            ui.weak(format!("（未対応の部品: {other}）"));
        }
    }
}

/// フォルダを OS のファイルマネージャーで開く。
fn open_in_file_manager(dir: &std::path::Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(dir).spawn().map(|_| ())
}
