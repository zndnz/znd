//! プラグインパッケージの実行側: コマンド・スクリプトのパネル・出力フィルター・安全性スキャンの結果と許可。
//!
//! 起動時に既定のプラグインフォルダを走査し（エフェクト・コマンド・パネル・出力フィルター・エクスプレッション用
//! ライブラリ・書き出し形式が使えるようになる）、画面からはコマンドの実行・パネルの組み立てと操作・管理を頼まれる。
//! スクリプトが `host.*` で頼んだ操作のうち、プロジェクトの書き換え・ファイルのダイアログ・音量の解析・
//! 出力フィルターの切り替えはここで行い、選択や再生ヘッドなど画面の状態に関わるものは画面へ返す。

use crate::{lock, Shared, Snapshot, Studio};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;
use znd_core::{CompId, LayerId};
use znd_media::{import_file, ImportOptions};
use znd_render::plugins::{self, hooks, scan::RiskLevel, Command, Panel, SecurityReport};

/// ファイルを読み書きするスクリプトに許す上限。
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// 1 回の呼び出しで続けて実行するコールバック・コマンドの上限（呼び合いで止まらなくならないように）。
const MAX_CHAIN: usize = 16;

#[derive(Default)]
pub struct Host {
    /// パネルごとの、スクリプトが持ち回る状態（`panel(state)` / `onEvent(..., state)` の state）。
    panel_state: HashMap<String, Value>,
    pub logs: Vec<String>,
    /// 前回の起動で、処理中に異常終了したプラグイン（種類名）。
    pub crash: Option<String>,
    /// 走査で読み込めなかったファイル。
    pub load_errors: Vec<(PathBuf, String)>,
}

impl Host {
    fn log(&mut self, line: String) {
        self.logs.push(line);
        let n = self.logs.len();
        if n > 200 {
            self.logs.drain(..n - 200);
        }
    }
}

/// 起動時に一度だけ呼ぶ。プラグインを読み込み、異常終了の記録を確かめてから今回の記録を始める。
pub fn startup(host: &mut Host, allow_native: bool) {
    // ネイティブのプラグインは、利用者が許可していなければ読み込まない（WebAssembly はサンドボックスで動く）
    plugins::set_native_allowed(allow_native);
    host.load_errors = plugins::scan_default().errors;
    host.crash = plugins::guard::take_crash_record();
    if let Err(e) = plugins::guard::install_crash_journal() {
        eprintln!("プラグインの異常終了の記録を始められません: {e}");
    }
}

/// スクリプトに渡す、画面の状態。
#[derive(Deserialize, Clone)]
pub struct Ctx {
    comp: String,
    layer: Option<String>,
    frame: i64,
}

/// スクリプトがどのパッケージのどのファイルから実行されたか（コールバックを同じスクリプトで呼ぶため）。
#[derive(Clone)]
struct Origin {
    label: String,
    plugin: String,
    dir: PathBuf,
    source: Arc<String>,
    /// パネルから呼ばれたときは、そのパネルのキー（コールバックが状態を返せるように）。
    panel: Option<String>,
}

/// 実行の結果。`actions` は画面が処理する操作（select / frame / time / work_area / status / alert / copy_text / comp / panel）。
#[derive(Serialize, Default)]
pub struct Outcome {
    snapshot: Option<Snapshot>,
    actions: Vec<Value>,
    status: Option<String>,
    /// プラグインの定義や出力フィルターが変わった（画面はメニュー・エフェクトの一覧を取り直す）。
    changed: bool,
}

fn context(s: &Studio, ctx: &Ctx, dir: &Path, plugin: &str) -> Value {
    let comp = s.project.comp(&CompId::new(ctx.comp.as_str()));
    let fr = comp.map(|c| c.frame_rate).unwrap_or(znd_core::FrameRate::FPS_30);
    json!({
        "comp": ctx.comp,
        "layer": ctx.layer,
        "layers": ctx.layer.iter().collect::<Vec<_>>(),
        "frame": ctx.frame,
        "time": fr.frame_to_time(ctx.frame).seconds(),
        "fps": fr.fps(),
        "plugin": { "id": plugin, "dir": dir.display().to_string() },
        "projectPath": s.path.as_ref().map(|p| p.display().to_string()),
    })
}

fn has_permission(plugin: &str, perm: &str) -> bool {
    plugins::packages().iter().any(|p| p.manifest.id == plugin && p.manifest.permissions.iter().any(|x| x.eq_ignore_ascii_case(perm)))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// テキストファイルの中身を文字列にする。UTF-8（BOM 付き含む）と UTF-16 を見分け、
/// どちらでもなければ Shift_JIS として読む（AviUtl 由来の字幕などに多い）。
pub fn decode_text(bytes: &[u8]) -> String {
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

/// レイヤーごとの、1 フレームごとの音の大きさ（RMS の dBFS。無音は -120）。
fn audio_levels(s: &Studio, comp_id: &CompId, layers: &[String]) -> Value {
    let Some(engine) = &s.engine else { return json!({}) };
    let Some(comp) = s.project.comp(comp_id) else { return json!({}) };
    let mixer = znd_audio::Mixer::new(&s.project, &*engine.clips);
    let fps = comp.frame_rate.fps();
    let total = mixer.comp_frames(comp);
    let frames = (comp.duration.seconds() * fps).round().max(0.0) as usize;
    let mut out = serde_json::Map::new();
    for id in layers.iter().take(64) {
        let lid = LayerId::new(id.as_str());
        if comp.layer(&lid).is_none() {
            continue;
        }
        let Ok(samples) = mixer.mix_layer(comp_id, &lid, 0, total) else { continue };
        let per = znd_audio::SAMPLE_RATE as f64 / fps;
        let v: Vec<f64> = (0..frames)
            .map(|f| {
                let (a, b) = (((f as f64 * per) as usize).min(total), (((f + 1) as f64 * per) as usize).min(total));
                if b <= a {
                    return -120.0;
                }
                let ms = samples[a * 2..b * 2].iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>() / ((b - a) * 2) as f64;
                if ms <= 1e-12 {
                    -120.0
                } else {
                    (10.0 * ms.log10()).max(-120.0)
                }
            })
            .collect();
        out.insert(id.clone(), json!(v));
    }
    Value::Object(out)
}

/// 続けて行う仕事（`host.runCommand` のコマンド、`host.openFile` などのコールバック）。
enum Next {
    Command(String),
    Call(Origin, String, Vec<Value>),
}

/// スクリプトを実行し、頼まれた操作を（続けて頼まれたものも含めて）すべて片付ける。
fn run(s: &mut Studio, ctx: &Ctx, first: Next, coalesce: bool) -> Outcome {
    let mut out = Outcome::default();
    let revision = s.revision;
    let mut queue = vec![first];
    let mut steps = 0;
    while let Some(next) = queue.pop() {
        steps += 1;
        if steps > MAX_CHAIN {
            out.status = Some("プラグインの呼び出しが多すぎるため、途中で止めました".into());
            break;
        }
        let (origin, call): (Origin, Option<(String, Vec<Value>)>) = match next {
            Next::Command(key) => match plugins::commands().into_iter().find(|c| c.key == key) {
                Some(cmd) => (command_origin(&cmd), cmd.function.clone().map(|f| (f, Vec::new()))),
                None => {
                    out.status = Some(format!("コマンド {key} がありません"));
                    continue;
                }
            },
            Next::Call(origin, f, args) => (origin, Some((f, args))),
        };
        let context = context(s, ctx, &origin.dir, &origin.plugin);
        let call_ref = call.as_ref().map(|(f, a)| (f.as_str(), a.as_slice()));
        match znd_script::run_plugin(&s.project, &origin.source, call_ref, &context) {
            Ok(r) => {
                // スクリプトが host.status で出したものがあれば、そちらが上書きする
                out.status.get_or_insert_with(|| format!("{} を実行しました", origin.label));
                // パネルのコールバックが返したオブジェクトは、そのパネルの新しい状態にする
                if let (Some(key), true) = (&origin.panel, r.ret.is_object()) {
                    s.plugins.panel_state.insert(key.clone(), r.ret.clone());
                }
                absorb(s, ctx, &origin, coalesce, r, &mut out, &mut queue);
            }
            Err(e) => {
                out.status = Some(format!("{}: {e}", origin.label));
                s.plugins.log(format!("[{}] エラー: {e}", origin.label));
            }
        }
    }
    if s.revision != revision {
        out.snapshot = Some(s.snapshot());
    }
    out
}

fn command_origin(cmd: &Command) -> Origin {
    Origin { label: cmd.label.clone(), plugin: cmd.plugin.clone(), dir: cmd.dir.clone(), source: cmd.source.clone(), panel: None }
}

/// スクリプトの結果を取り込む（プロジェクトの書き換え・ログ・頼まれた操作）。
fn absorb(s: &mut Studio, ctx: &Ctx, origin: &Origin, coalesce: bool, run: znd_script::PluginRun, out: &mut Outcome, queue: &mut Vec<Next>) {
    let label = origin.label.clone();
    if let Some(updated) = run.project {
        let comp = CompId::new(ctx.comp.as_str());
        match s.project.comp(&comp).and_then(|c| c.locked_by.clone()).filter(|o| o != &s.user) {
            Some(owner) => out.status = Some(format!("このコンポは {owner} がロックしているため、プラグインの変更を取り込みませんでした")),
            None => s.apply(updated, &format!("プラグイン: {label}"), coalesce),
        }
    }
    for l in run.logs {
        s.plugins.log(format!("[{label}] {l}"));
    }
    for a in run.actions {
        let kind = a.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        match kind.as_str() {
            "status" => out.status = a.get("text").and_then(Value::as_str).map(String::from),
            "filter" => {
                hooks::set_filter_enabled(a.get("key").and_then(Value::as_str).unwrap_or(""), a.get("on").and_then(Value::as_bool).unwrap_or(false));
                s.refresh();
                out.changed = true;
            }
            "open_project_folder" => match s.path.as_ref().and_then(|p| p.parent()) {
                Some(dir) => {
                    if let Err(e) = crate::open_in_file_manager(dir) {
                        out.status = Some(format!("{} を開けません: {e}", dir.display()));
                    }
                }
                None => out.status = Some("プロジェクトがまだ保存されていません".into()),
            },
            "audio_levels" => {
                let callback = a.get("callback").and_then(Value::as_str).unwrap_or("").to_string();
                let comp_id = a.get("comp").and_then(Value::as_str).map(CompId::new).unwrap_or_else(|| CompId::new(ctx.comp.as_str()));
                let levels = audio_levels(s, &comp_id, &strings(a.get("layers")));
                let data = a.get("data").cloned().unwrap_or(Value::Null);
                if !callback.is_empty() {
                    queue.push(Next::Call(origin.clone(), callback, vec![levels, data]));
                }
            }
            "command" => {
                if let Some(k) = a.get("key").and_then(Value::as_str) {
                    queue.push(Next::Command(k.to_string()));
                }
            }
            "open_file" | "save_file" | "import" => file_action(s, ctx, origin, &kind, &a, out, queue),
            // 選択・再生ヘッド・ワークエリア・お知らせ・クリップボード・コンポの切り替え・パネルの開閉は画面が行う
            _ => out.actions.push(a),
        }
    }
}

fn file_action(s: &mut Studio, ctx: &Ctx, origin: &Origin, kind: &str, a: &Value, out: &mut Outcome, queue: &mut Vec<Next>) {
    let label = &origin.label;
    if !has_permission(&origin.plugin, "filesystem") {
        let msg = format!("[{label}] ファイルを扱うには plugin.json の permissions に \"filesystem\" が必要です");
        out.status = Some(msg.clone());
        s.plugins.log(msg);
        return;
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
            let Some(path) = dialog.pick_file() else { return };
            if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                out.status = Some(format!("[{label}] ファイルが大きすぎます（64MB まで）"));
                return;
            }
            let content = match std::fs::read(&path) {
                Ok(bytes) if a.get("binary").and_then(Value::as_bool).unwrap_or(false) => json!(bytes),
                Ok(bytes) => json!(decode_text(&bytes)),
                Err(e) => {
                    out.status = Some(format!("[{label}] 読めません: {e}"));
                    return;
                }
            };
            let callback = a.get("callback").and_then(Value::as_str).unwrap_or("").to_string();
            let data = a.get("data").cloned().unwrap_or(Value::Null);
            if !callback.is_empty() {
                queue.push(Next::Call(origin.clone(), callback, vec![content, json!(path.display().to_string()), data]));
            }
        }
        "save_file" => {
            let name = a.get("name").and_then(Value::as_str).unwrap_or("untitled");
            let Some(path) = dialog.set_file_name(name).save_file() else { return };
            let bytes: Vec<u8> = match a.get("content") {
                Some(Value::String(t)) => t.as_bytes().to_vec(),
                Some(Value::Array(v)) => v.iter().map(|b| b.as_u64().unwrap_or(0) as u8).collect(),
                _ => Vec::new(),
            };
            out.status = Some(match std::fs::write(&path, bytes) {
                Ok(()) => format!("{} に保存しました", path.display()),
                Err(e) => format!("[{label}] 保存できません: {e}"),
            });
        }
        _ => {
            let sequence = a.get("sequence").and_then(Value::as_bool).unwrap_or(false);
            let comp = CompId::new(ctx.comp.as_str());
            let Some(fr) = s.project.comp(&comp).map(|c| c.frame_rate) else { return };
            for path in dialog.pick_files().unwrap_or_default() {
                let mut project = (*s.project).clone();
                let opts = ImportOptions { sequence, at: fr.frame_to_time(ctx.frame), ..Default::default() };
                match import_file(&mut project, &comp, &path, opts) {
                    Ok(_) => s.apply(project, "素材の読み込み", false),
                    Err(e) => out.status = Some(format!("読み込めません: {e}")),
                }
            }
        }
    }
}

// ---------- 画面から呼ぶもの ----------

#[derive(Serialize)]
struct CommandInfo {
    key: String,
    plugin: String,
    label: String,
    description: String,
    /// "plugins" / "layer" / "toolbar"
    menu: Vec<String>,
    shortcut: Option<String>,
}

#[derive(Serialize)]
struct PanelInfo {
    key: String,
    plugin: String,
    title: String,
    width: f32,
}

#[derive(Serialize)]
struct FilterInfo {
    key: String,
    label: String,
    stage: &'static str,
    enabled: bool,
}

#[derive(Serialize)]
struct FindingInfo {
    category: &'static str,
    score: u32,
    title: String,
    evidence: Vec<String>,
}

#[derive(Serialize)]
struct ReportInfo {
    level: &'static str,
    /// "safe" / "low" / "medium" / "high" / "critical"
    risk: &'static str,
    blocks: bool,
    summary: String,
    findings: Vec<FindingInfo>,
    endpoints: Vec<String>,
    hash: String,
    files_scanned: usize,
}

fn report_info(r: &SecurityReport) -> ReportInfo {
    ReportInfo {
        level: r.level.short(),
        risk: match r.level {
            RiskLevel::Safe => "safe",
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        },
        blocks: r.level.blocks(),
        summary: r.summary(),
        findings: r.findings.iter().map(|f| FindingInfo { category: f.category.label(), score: f.score, title: f.title.clone(), evidence: f.evidence.clone() }).collect(),
        endpoints: r.endpoints.clone(),
        hash: r.hash.clone(),
        files_scanned: r.files_scanned,
    }
}

#[derive(Serialize)]
struct PackageInfo {
    id: String,
    name: String,
    version: String,
    vendor: String,
    license: String,
    description: String,
    permissions: Vec<String>,
    dir: String,
    commands: usize,
    panels: usize,
    filters: usize,
    expressions: usize,
    exporters: usize,
    effects: usize,
    failed: Option<String>,
    warnings: Vec<String>,
    report: ReportInfo,
}

#[derive(Serialize)]
struct EffectPluginInfo {
    kind: String,
    label: String,
    category: String,
    api: &'static str,
    vendor: String,
    version: String,
    license: String,
    path: String,
    params: usize,
    report: Option<ReportInfo>,
}

#[derive(Serialize)]
struct BlockedInfo {
    path: String,
    name: String,
    report: ReportInfo,
}

#[derive(Serialize)]
struct FaultInfo {
    kind: String,
    why: String,
    quarantined: bool,
    disabled: bool,
}

/// プラグインの状態をまとめたもの（メニュー・ツールバー・管理画面が使う）。
#[derive(Serialize)]
pub struct PluginState {
    commands: Vec<CommandInfo>,
    panels: Vec<PanelInfo>,
    filters: Vec<FilterInfo>,
    packages: Vec<PackageInfo>,
    effects: Vec<EffectPluginInfo>,
    blocked: Vec<BlockedInfo>,
    disabled: Vec<String>,
    faults: Vec<FaultInfo>,
    /// このプロジェクトが使っているのに入っていないもの（種類, レイヤー名）。
    missing: Vec<(String, String)>,
    errors: Vec<(String, String)>,
    logs: Vec<String>,
    ofx_messages: Vec<String>,
    native_allowed: bool,
    plugin_dir: Option<String>,
    /// 前回の起動で異常終了したプラグインの（種類, 名前）。
    crash: Option<(String, String)>,
    disclaimer: &'static str,
}

pub fn state_of(s: &Studio) -> PluginState {
    let commands = plugins::commands()
        .into_iter()
        .map(|c| CommandInfo {
            key: c.key,
            plugin: c.plugin_name,
            label: c.label,
            description: c.description,
            menu: if c.menu.is_empty() { vec!["plugins".into()] } else { c.menu },
            shortcut: c.shortcut,
        })
        .collect();
    let panels = plugins::panels().into_iter().map(|p| PanelInfo { key: p.key, plugin: p.plugin, title: p.title, width: p.width }).collect();
    let filters = hooks::output_filters().into_iter().map(|f| FilterInfo { key: f.key, label: f.label, stage: f.stage.label(), enabled: f.enabled }).collect();
    let packages = plugins::packages()
        .iter()
        .filter(|p| p.status != plugins::PackageStatus::Blocked)
        .map(|p| {
            let m = &p.manifest;
            PackageInfo {
                id: m.id.clone(),
                name: m.name.clone(),
                version: m.version.clone(),
                vendor: m.vendor.clone(),
                license: m.license.clone(),
                description: m.description.clone(),
                permissions: m.permissions.clone(),
                dir: p.dir.display().to_string(),
                commands: p.commands.len(),
                panels: p.panels.len(),
                filters: m.output_filters.len(),
                expressions: m.expressions.len(),
                exporters: m.exporters.len(),
                effects: p.effect_count,
                failed: match &p.status {
                    plugins::PackageStatus::Failed(e) => Some(e.clone()),
                    _ => None,
                },
                warnings: p.warnings.clone(),
                report: report_info(&p.report),
            }
        })
        .collect();
    let effects = plugins::loaded()
        .into_iter()
        .map(|i| EffectPluginInfo {
            report: i.report().map(|r| report_info(&r)),
            kind: i.kind,
            label: i.label,
            category: i.category,
            api: i.api.label(),
            vendor: i.vendor,
            version: i.version,
            license: i.license,
            path: i.path.display().to_string(),
            params: i.param_count,
        })
        .collect();
    let blocked = plugins::blocked()
        .into_iter()
        .map(|(p, r)| BlockedInfo {
            name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: p.display().to_string(),
            report: report_info(&r),
        })
        .collect();
    let faults = plugins::guard::faults()
        .into_iter()
        .map(|(kind, why)| FaultInfo { quarantined: plugins::guard::is_quarantined(&kind), disabled: plugins::guard::is_disabled(&kind), kind, why })
        .collect();
    let crash = s.plugins.crash.clone().map(|kind| {
        let label = plugins::loaded().into_iter().find(|i| i.kind == kind).map(|i| i.label).unwrap_or_else(|| kind.clone());
        (kind, label)
    });
    PluginState {
        commands,
        panels,
        filters,
        packages,
        effects,
        blocked,
        disabled: plugins::guard::disabled(),
        faults,
        missing: plugins::missing(&s.project),
        errors: s
            .plugins
            .load_errors
            .iter()
            .filter(|(_, e)| !e.starts_with(plugins::BLOCKED_PREFIX))
            .map(|(p, e)| (p.display().to_string(), e.clone()))
            .collect(),
        logs: s.plugins.logs.clone(),
        ofx_messages: plugins::ofx::recent_messages(),
        native_allowed: plugins::native_allowed(),
        plugin_dir: plugins::user_plugin_dir().map(|d| d.display().to_string()),
        crash,
        disclaimer: plugins::DISCLAIMER,
    }
}

#[tauri::command]
pub fn plugin_state(state: State<'_, Shared>) -> PluginState {
    state_of(&lock(&state))
}

#[tauri::command]
pub fn plugin_command(state: State<'_, Shared>, key: String, ctx: Ctx) -> Outcome {
    let mut s = lock(&state);
    run(&mut s, &ctx, Next::Command(key), false)
}

#[derive(Serialize)]
pub struct PanelBuild {
    items: Vec<Value>,
    error: Option<String>,
}

fn find_panel(key: &str) -> Result<Panel, String> {
    plugins::panels().into_iter().find(|p| p.key == key).ok_or_else(|| format!("パネル {key} がありません"))
}

fn panel_origin(panel: &Panel, label: String) -> Origin {
    Origin { label, plugin: panel.plugin.clone(), dir: panel.dir.clone(), source: panel.source.clone(), panel: Some(panel.key.clone()) }
}

/// スクリプトのパネルの部品を作る（`panel(state)` の戻り値）。プロジェクトの書き換えは受け付けない。
#[tauri::command]
pub fn plugin_panel(state: State<'_, Shared>, key: String, ctx: Ctx) -> Result<PanelBuild, String> {
    build_panel(&mut lock(&state), key, &ctx)
}

fn build_panel(s: &mut Studio, key: String, ctx: &Ctx) -> Result<PanelBuild, String> {
    let panel = find_panel(&key)?;
    let context = context(s, ctx, &panel.dir, &panel.plugin);
    let st = s.plugins.panel_state.get(&key).cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    // panel(state) は state を書き換えてもよい（既定値の設定など）ので、戻り値と一緒に state も受け取る
    let wrapper = format!("{}\n;function __panel(state) {{ var items = panel(state); return {{ items: items, state: state }}; }}", panel.source);
    match znd_script::run_plugin(&s.project, &wrapper, Some(("__panel", &[st])), &context) {
        Ok(run) => {
            if let Some(st) = run.ret.get("state").filter(|v| v.is_object()) {
                s.plugins.panel_state.insert(key, st.clone());
            }
            Ok(PanelBuild { items: run.ret.get("items").and_then(Value::as_array).cloned().unwrap_or_default(), error: None })
        }
        Err(e) => Ok(PanelBuild { items: Vec::new(), error: Some(e) }),
    }
}

/// パネルの部品の操作を `onEvent(id, value, state)` へ送る。
#[tauri::command]
pub fn plugin_panel_event(state: State<'_, Shared>, key: String, id: String, value: Value, ctx: Ctx) -> Result<Outcome, String> {
    panel_event(&mut lock(&state), &key, &id, value, &ctx)
}

fn panel_event(s: &mut Studio, key: &str, id: &str, value: Value, ctx: &Ctx) -> Result<Outcome, String> {
    let panel = find_panel(key)?;
    let st = s.plugins.panel_state.get(key).cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
    let wrapper = format!(
        "{}\n;function __event(id, value, state) {{ if (typeof onEvent === 'function') onEvent(id, value, state); return state; }}",
        panel.source
    );
    let mut origin = panel_origin(&panel, format!("{} / {id}", panel.title));
    origin.source = Arc::new(wrapper);
    // スライダーのドラッグなど、続けて届く同じ部品の操作は 1 つの取り消し単位にまとめる
    let mut out = run(s, ctx, Next::Call(origin, "__event".into(), vec![json!(id), value, st]), true);
    // 部品の操作で出る「実行しました」は煩いので出さない
    if out.status.as_deref().is_some_and(|t| t.ends_with("を実行しました")) {
        out.status = None;
    }
    Ok(out)
}

// ---------- 管理 ----------

/// プラグインの定義が変わったあとの共通処理（プレビューを作り直し、メニューを作り直す）。
fn changed(s: &mut Studio) -> Snapshot {
    s.refresh();
    s.rebuild_menu();
    s.snapshot()
}

#[derive(Serialize)]
pub struct Managed {
    snapshot: Snapshot,
    status: String,
}

/// ファイル（.wasm など）かフォルダ（パッケージ・OpenFX の .ofx.bundle）を選んで読み込む。
#[tauri::command]
pub fn plugin_load(state: State<'_, Shared>, folder: bool) -> Option<Managed> {
    let picked = if folder {
        rfd::FileDialog::new().pick_folder()
    } else {
        rfd::FileDialog::new().add_filter("プラグイン", &["wasm", "dylib", "so", "dll", "ofx", "bundle"]).pick_file()
    }?;
    let mut s = lock(&state);
    let status = match plugins::load_plugin_file(&picked) {
        Ok(infos) => {
            let r = plugins::report_for(&picked).map(|r| format!("・危険度 {}", r.level.short())).unwrap_or_default();
            format!("{} を読み込みました（エフェクト {} 個{r}）", picked.display(), infos.len())
        }
        Err(e) => format!("プラグインを読み込めません: {e}"),
    };
    Some(Managed { snapshot: changed(&mut s), status })
}

#[tauri::command]
pub fn plugin_rescan(state: State<'_, Shared>) -> Managed {
    let mut s = lock(&state);
    let report = plugins::scan_default();
    s.plugins.load_errors = plugins::load_errors();
    let status = format!("プラグインを走査しました（新しく {} 個）", report.loaded.len());
    Managed { snapshot: changed(&mut s), status }
}

/// 危険度が高いと判定したものを、利用者の判断で許可して読み込む。
#[tauri::command]
pub fn plugin_allow(state: State<'_, Shared>, path: String) -> Managed {
    let mut s = lock(&state);
    let path = PathBuf::from(path);
    let status = match plugins::allow_and_load(&path) {
        Ok(infos) => format!("自己責任で許可して読み込みました: {}（エフェクト {} 個）", path.display(), infos.len()),
        Err(e) => format!("読み込めません: {e}"),
    };
    s.plugins.load_errors = plugins::load_errors();
    Managed { snapshot: changed(&mut s), status }
}

#[tauri::command]
pub fn plugin_revoke(path: String) -> String {
    match plugins::revoke(Path::new(&path)) {
        Ok(()) => "許可を取り消しました（次の起動から止めます）".into(),
        Err(e) => e,
    }
}

#[tauri::command]
pub fn plugin_set_native(state: State<'_, Shared>, on: bool) -> String {
    let mut s = lock(&state);
    plugins::set_native_allowed(on);
    s.prefs.set_allow_native_plugins(on);
    match s.prefs.save() {
        Err(e) => format!("設定を保存できません: {e}"),
        Ok(()) if on => "ネイティブのプラグインを許可しました（自己責任）。再走査すると読み込みます".into(),
        Ok(()) => "ネイティブのプラグインを止めました。次の起動から読み込みません".into(),
    }
}

/// エフェクトのプラグインを無効にする・戻す（無効のあいだは元の画像をそのまま通す）。
#[tauri::command]
pub fn plugin_set_disabled(state: State<'_, Shared>, kind: String, off: bool) -> Result<Snapshot, String> {
    plugins::guard::set_disabled(&kind, off)?;
    let mut s = lock(&state);
    Ok(changed(&mut s))
}

/// 不具合で止めたエフェクトを、もう一度使う。
#[tauri::command]
pub fn plugin_release(state: State<'_, Shared>, kind: String) -> Snapshot {
    plugins::guard::release(&kind);
    let mut s = lock(&state);
    changed(&mut s)
}

/// 前回の異常終了について尋ねた結果（`disable` なら無効にする）。
#[tauri::command]
pub fn plugin_crash_answer(state: State<'_, Shared>, disable: bool) -> Result<Snapshot, String> {
    let mut s = lock(&state);
    let kind = s.plugins.crash.take();
    if let (true, Some(kind)) = (disable, kind) {
        plugins::guard::set_disabled(&kind, true)?;
    }
    Ok(changed(&mut s))
}

#[tauri::command]
pub fn plugin_set_filter(state: State<'_, Shared>, key: String, on: bool) -> Snapshot {
    hooks::set_filter_enabled(&key, on);
    let mut s = lock(&state);
    changed(&mut s)
}

#[tauri::command]
pub fn plugin_open_dir() -> Result<(), String> {
    let dir = plugins::user_plugin_dir().ok_or("プラグインフォルダが分かりません")?;
    let _ = std::fs::create_dir_all(&dir);
    crate::open_in_file_manager(&dir).map_err(|e| format!("{} を開けません: {e}", dir.display()))
}

#[tauri::command]
pub fn plugin_clear_logs(state: State<'_, Shared>) {
    lock(&state).plugins.logs.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::*;

    fn studio_with_layer() -> Studio {
        let mut p = Project::new("t");
        let mut c = Composition::new("c", "c", 400, 200, FrameRate::FPS_30, Time::from_seconds(2.0));
        c.layers.push(Layer::new("l", "平面", LayerKind::Solid { width: 10, height: 10, color: [1.0; 3] }, Transform::centered(10.0, 10.0, 40.0, 40.0), c.duration));
        p.compositions.push(c);
        Studio::new(p, None)
    }

    fn toolkit() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/toolkit.zndplugin");
        plugins::load_plugin_file(&dir).expect("サンプルのパッケージを読み込めない");
        dir
    }

    /// サンプルのパッケージのコマンドを実行すると、プロジェクトが書き換わり、取り消せる。
    #[test]
    fn commands_from_a_package_edit_the_project() {
        toolkit();
        let mut s = studio_with_layer();
        let ctx = Ctx { comp: "c".into(), layer: Some("l".into()), frame: 0 };
        let out = run(&mut s, &ctx, Next::Command("com.example.toolkit/center".into()), false);
        let snap = out.snapshot.expect("プロジェクトが書き換わっていない");
        let pos = snap.project.comp(&CompId::new("c")).unwrap().layer(&LayerId::new("l")).unwrap().transform.position.value;
        assert_eq!(pos, [200.0, 100.0]);
        assert!(out.status.unwrap().contains("中央"));
        assert_eq!(s.undo.len(), 1);
    }

    /// パネルは部品を返し、部品の操作でプロジェクトを書き換えられる。状態はパネルごとに保たれる。
    #[test]
    fn panels_build_items_and_handle_events() {
        toolkit();
        let mut s = studio_with_layer();
        let ctx = Ctx { comp: "c".into(), layer: Some("l".into()), frame: 0 };
        let key = "com.example.toolkit/motion".to_string();
        let built = build_panel(&mut s, key.clone(), &ctx).unwrap();
        assert!(built.error.is_none(), "{:?}", built.error);
        assert!(built.items.iter().any(|i| i["type"] == "slider"));
        assert_eq!(s.plugins.panel_state[&key]["amp"], 20);
        let out = panel_event(&mut s, &key, "amp", json!(50), &ctx).unwrap();
        assert!(out.snapshot.is_none(), "プロジェクトを書き換えていないのに編集になった");
        assert_eq!(s.plugins.panel_state[&key]["amp"], 50);
        let out = panel_event(&mut s, &key, "apply", Value::Null, &ctx).unwrap();
        let snap = out.snapshot.expect("位置に付けていない");
        let l = snap.project.comp(&CompId::new("c")).unwrap().layer(&LayerId::new("l")).unwrap().clone();
        assert!(l.transform.position.expression.is_some());
    }

    #[test]
    fn text_files_are_decoded_from_common_encodings() {
        assert_eq!(decode_text("あ".as_bytes()), "あ");
        assert_eq!(decode_text(&[0xef, 0xbb, 0xbf, b'a']), "a");
        assert_eq!(decode_text(&[0xff, 0xfe, 0x42, 0x30]), "あ");
        // Shift_JIS の「あ」
        assert_eq!(decode_text(&[0x82, 0xa0]), "あ");
    }
}
