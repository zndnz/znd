//! プラグイン（外部の動的ライブラリが提供するエフェクト）。
//!
//! 2 種類に対応する:
//! - znd ネイティブ API（`sdk/znd_plugin.h`）: エフェクトの種類名は `plugin:<id>`
//! - OpenFX 画像エフェクト（`.ofx.bundle`）: エフェクトの種類名は `ofx:<識別子>`
//!
//! 読み込んだエフェクトは `znd_core::effect::register_effect` で定義表に加わるので、
//! 組み込みエフェクトと同じようにメニューに並び、パラメータにキーフレームやエクスプレッションを付けられる。
//! プロジェクトにはエフェクトの種類名とパラメータだけが保存される。プラグインが無い環境では
//! そのエフェクトは何もせず（元の画像のまま）、`missing` で検出できる。
//!
//! プラグインはホストと同じプロセス・同じ権限で動く。信頼できるものだけを入れること。
//! 読み込む前に `scan` で静的な安全性スキャンを行い、危険度が高いものは利用者が
//! 自己責任で許可（`trust`）するまで読み込まない。
//!
//! `plugin.json` を持つフォルダ（パッケージ、`package`）は、エフェクトに加えてコマンド・パネル・
//! 出力フィルター・エクスプレッション用ライブラリを提供できる（`hooks`）。

pub mod guard;
pub mod hooks;
pub mod native;
pub mod ofx;
pub mod package;
pub mod scan;
pub mod trust;

pub use package::{Command, Package, PackageStatus, Panel};
pub use scan::{RiskLevel, SecurityReport, DISCLAIMER};

use crate::effects::FxContext;
use crate::frame::FrameBuf;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use znd_core::effect::EvalParams;
use znd_core::Project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginApi {
    Native,
    OpenFx,
}

impl PluginApi {
    pub fn label(self) -> &'static str {
        match self {
            PluginApi::Native => "znd",
            PluginApi::OpenFx => "OpenFX",
        }
    }
}

/// 読み込んだエフェクト 1 つの情報（一覧表示用）。
#[derive(Debug, Clone, PartialEq)]
pub struct PluginInfo {
    /// エフェクトの種類名（`plugin:...` / `ofx:...`）。
    pub kind: String,
    pub label: String,
    pub category: String,
    pub vendor: String,
    pub version: String,
    pub license: String,
    pub path: PathBuf,
    pub api: PluginApi,
    pub param_count: usize,
}

impl PluginInfo {
    /// このエフェクトを含むファイルの安全性スキャンの結果。
    pub fn report(&self) -> Option<SecurityReport> {
        report_for(&self.path)
    }
}

pub(crate) trait Effect: Send + Sync {
    fn info(&self) -> &PluginInfo;
    fn apply(&self, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf;
}

#[derive(Default)]
struct Registry {
    effects: BTreeMap<String, Arc<dyn Effect>>,
    /// 読み込み済み（または失敗済み）のファイル。
    files: BTreeMap<PathBuf, Result<usize, String>>,
    /// 調べたファイル・パッケージの安全性スキャンの結果。
    reports: BTreeMap<PathBuf, SecurityReport>,
    /// 危険度が高いため読み込みを止めているもの。
    blocked: BTreeMap<PathBuf, SecurityReport>,
    packages: BTreeMap<PathBuf, Arc<Package>>,
}

/// 読み込みを止めたときのエラーの先頭に付ける印。
pub const BLOCKED_PREFIX: &str = "安全性スキャンで危険度";

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// `kind` がプラグインのエフェクトか。
pub fn is_plugin_kind(kind: &str) -> bool {
    kind.starts_with("plugin:") || kind.starts_with("ofx:")
}

/// プラグインを探すフォルダ（存在しないものも含む。優先順）。
///
/// プロジェクトファイルの隣のフォルダは含めない。受け取ったプロジェクトを開いただけで、
/// 同梱されたライブラリが実行されてしまうのを避けるため。
pub fn plugin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(list) = std::env::var_os("ZND_PLUGIN_PATH") {
        dirs.extend(std::env::split_paths(&list));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if cfg!(target_os = "macos") {
            dirs.push(home.join("Library/Application Support/znd/plugins"));
        } else {
            dirs.push(home.join(".local/share/znd/plugins"));
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        dirs.push(appdata.join("znd").join("plugins"));
    }
    // OpenFX の標準の置き場所
    if let Some(list) = std::env::var_os("OFX_PLUGIN_PATH") {
        dirs.extend(std::env::split_paths(&list));
    }
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/Library/OFX/Plugins"));
    } else if cfg!(target_os = "windows") {
        dirs.push(PathBuf::from(r"C:\Program Files\Common Files\OFX\Plugins"));
    } else {
        dirs.push(PathBuf::from("/usr/OFX/Plugins"));
    }
    dirs
}

/// ユーザーがプラグインを置く既定のフォルダ。
pub fn user_plugin_dir() -> Option<PathBuf> {
    plugin_dirs().into_iter().find(|d| d.ends_with("znd/plugins") || d.ends_with("znd\\plugins"))
}

fn is_native_lib(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("dylib" | "so" | "dll"))
}

fn is_ofx(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.ends_with(".ofx.bundle") || name.ends_with(".ofx")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanReport {
    /// 今回新しく読み込んだエフェクト。
    pub loaded: Vec<PluginInfo>,
    pub errors: Vec<(PathBuf, String)>,
}

/// プラグインファイル（ネイティブのライブラリ、または `.ofx.bundle` / `.ofx`）か、パッケージのフォルダを
/// 1 つ読み込んで登録する。既に読み込み済みなら何もしない。
///
/// 読み込む前に安全性スキャンを行い、危険度が「高」以上で利用者の許可が無ければ読み込まずにエラーを返す
/// （`blocked` に載る。`allow_and_load` で自己責任で読み込める）。
pub fn load_plugin_file(path: &Path) -> Result<Vec<PluginInfo>, String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if package::is_package(&key) {
        let pkg = load_package(&key);
        return match &pkg.status {
            PackageStatus::Loaded => Ok(Vec::new()),
            PackageStatus::Blocked => Err(blocked_message(&pkg.report)),
            PackageStatus::Failed(e) => Err(e.clone()),
        };
    }
    if let Some(prev) = registry().lock().unwrap().files.get(&key) {
        return prev.clone().map(|_| Vec::new());
    }
    let report = scan::scan_path(&key, &scan::Declared::default());
    let blocked = report.level.blocks() && !trust::is_trusted(&report.hash);
    {
        let mut reg = registry().lock().unwrap();
        reg.reports.insert(key.clone(), report.clone());
        if blocked {
            reg.blocked.insert(key.clone(), report.clone());
            return Err(blocked_message(&report));
        }
        reg.blocked.remove(&key);
    }
    load_unchecked(path)
}

fn blocked_message(report: &SecurityReport) -> String {
    format!(
        "{}「{}」のため読み込みを止めました（{}）。内容を確認し、自己責任で許可した場合のみ読み込めます",
        BLOCKED_PREFIX,
        report.level.short(),
        report.findings.first().map(|f| f.title.as_str()).unwrap_or("")
    )
}

/// スキャンを済ませたファイルを読み込む（パッケージ内のエフェクト用）。
pub(crate) fn load_unchecked(path: &Path) -> Result<Vec<PluginInfo>, String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(prev) = registry().lock().unwrap().files.get(&key) {
        return prev.clone().map(|_| Vec::new());
    }
    let result: Result<Vec<Arc<dyn Effect>>, String> = if is_ofx(path) {
        ofx::load(path)
    } else if is_native_lib(path) {
        native::load(path)
    } else {
        Err("プラグインとして読み込める形式ではありません（.dylib / .so / .dll / .ofx.bundle / plugin.json のあるフォルダ）".into())
    };
    let mut reg = registry().lock().unwrap();
    match result {
        Ok(effects) => {
            let mut infos = Vec::new();
            for e in effects {
                let info = e.info().clone();
                // 先に見つかったものを優先する（同じエフェクトが複数の場所にある場合）
                if !reg.effects.contains_key(&info.kind) {
                    reg.effects.insert(info.kind.clone(), e);
                    infos.push(info);
                }
            }
            reg.files.insert(key, Ok(infos.len()));
            Ok(infos)
        }
        Err(e) => {
            reg.files.insert(key, Err(e.clone()));
            Err(e)
        }
    }
}

fn load_package(dir: &Path) -> Arc<Package> {
    if let Some(p) = registry().lock().unwrap().packages.get(dir).filter(|p| p.status == PackageStatus::Loaded) {
        return p.clone();
    }
    let pkg = Arc::new(package::load(dir));
    let mut reg = registry().lock().unwrap();
    reg.reports.insert(dir.to_path_buf(), pkg.report.clone());
    if pkg.status == PackageStatus::Blocked {
        reg.blocked.insert(dir.to_path_buf(), pkg.report.clone());
    } else {
        reg.blocked.remove(dir);
    }
    reg.packages.insert(dir.to_path_buf(), pkg.clone());
    pkg
}

/// 止めていたプラグインを、利用者の自己責任で許可して読み込む。
/// スキャンし直した結果のハッシュを許可として記録するので、中身が変わると再び止まる。
pub fn allow_and_load(path: &Path) -> Result<Vec<PluginInfo>, String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let report = if package::is_package(&key) {
        let m = package::read_manifest(&key)?;
        scan::scan_path(&key, &package::declared(&key, &m))
    } else {
        scan::scan_path(&key, &scan::Declared::default())
    };
    trust::allow(&report.hash, &key, report.level.short())?;
    {
        let mut reg = registry().lock().unwrap();
        reg.blocked.remove(&key);
        reg.packages.remove(&key);
        reg.files.remove(&key);
    }
    load_plugin_file(path)
}

/// 許可を取り消す（読み込み済みのものは、次の起動から止まる）。
pub fn revoke(path: &Path) -> Result<(), String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let hash = registry().lock().unwrap().reports.get(&key).map(|r| r.hash.clone());
    match hash {
        Some(h) => trust::revoke(&h),
        None => Err("このプラグインのスキャン結果がありません".into()),
    }
}

/// 読み込みを止めているもの（パス, スキャン結果）。
pub fn blocked() -> Vec<(PathBuf, SecurityReport)> {
    registry().lock().unwrap().blocked.iter().map(|(p, r)| (p.clone(), r.clone())).collect()
}

/// そのファイル・パッケージのスキャン結果。
pub fn report_for(path: &Path) -> Option<SecurityReport> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let reg = registry().lock().unwrap();
    reg.reports.get(&key).cloned().or_else(|| {
        // バンドルの中のファイルやパッケージ内のライブラリは、その親のスキャン結果を返す
        reg.reports.iter().filter(|(p, _)| key.starts_with(p)).max_by_key(|(p, _)| p.as_os_str().len()).map(|(_, r)| r.clone())
    })
}

/// 読み込んだ（または止めた・失敗した）パッケージ。
pub fn packages() -> Vec<Arc<Package>> {
    registry().lock().unwrap().packages.values().cloned().collect()
}

/// 読み込み済みのパッケージが提供するコマンド。
pub fn commands() -> Vec<Command> {
    packages().iter().filter(|p| p.status == PackageStatus::Loaded).flat_map(|p| p.commands.clone()).collect()
}

/// 読み込み済みのパッケージが提供するパネル。
pub fn panels() -> Vec<Panel> {
    packages().iter().filter(|p| p.status == PackageStatus::Loaded).flat_map(|p| p.panels.clone()).collect()
}

/// フォルダの中のプラグインをすべて読み込む（フォルダ直下と、1 段下のフォルダまで）。
/// `plugin.json` のあるフォルダはパッケージとして読み込み、その中は個別に探さない。
pub fn scan(dirs: &[PathBuf]) -> ScanReport {
    let mut report = ScanReport::default();
    let visit = |path: &Path, report: &mut ScanReport| {
        if !(package::is_package(path) || is_ofx(path) || (is_native_lib(path) && path.is_file())) {
            return false;
        }
        match load_plugin_file(path) {
            Ok(infos) => report.loaded.extend(infos),
            Err(e) => {
                if !report.errors.iter().any(|(p, _)| p == path) {
                    report.errors.push((path.to_path_buf(), e));
                }
            }
        }
        true
    };
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if visit(&path, &mut report) || !path.is_dir() {
                continue;
            }
            let Ok(sub) = std::fs::read_dir(&path) else { continue };
            let mut inner: Vec<PathBuf> = sub.flatten().map(|e| e.path()).collect();
            inner.sort();
            for p in inner {
                visit(&p, &mut report);
            }
        }
    }
    report
}

/// 標準の場所を走査する。
pub fn scan_default() -> ScanReport {
    scan(&plugin_dirs())
}

/// 読み込み済みのエフェクトの一覧（種類名の順）。
pub fn loaded() -> Vec<PluginInfo> {
    registry().lock().unwrap().effects.values().map(|e| e.info().clone()).collect()
}

/// 読み込みに失敗したファイルと理由。
pub fn load_errors() -> Vec<(PathBuf, String)> {
    registry().lock().unwrap().files.iter().filter_map(|(p, r)| r.as_ref().err().map(|e| (p.clone(), e.clone()))).collect()
}

/// プロジェクトで使われているのに読み込まれていないプラグインエフェクト（種類名, 使っているレイヤー名）。
pub fn missing(project: &Project) -> Vec<(String, String)> {
    let reg = registry().lock().unwrap();
    let mut out = Vec::new();
    for comp in &project.compositions {
        for layer in &comp.layers {
            for fx in &layer.effects {
                if is_plugin_kind(&fx.kind) && !reg.effects.contains_key(&fx.kind) {
                    out.push((fx.kind.clone(), format!("{} / {}", comp.name, layer.name)));
                }
            }
        }
    }
    out
}

/// プラグインのエフェクトを適用する。読み込まれていない・無効にした・不具合で止めたものは元の画像のまま返す。
pub fn apply(kind: &str, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let effect = registry().lock().unwrap().effects.get(kind).cloned();
    match effect {
        Some(e) => {
            // パニックしたら元の画像を通す（そのために、入力を先に複製しておく）
            let fallback = buf.clone();
            let mut out = guard::call(kind, || fallback, || e.apply(p, buf, ctx));
            // アルファが 0..1 を外れると後段の合成が破綻するので、ここで収める
            out.data.chunks_exact_mut(4).for_each(|px| px[3] = px[3].clamp(0.0, 1.0));
            out
        }
        None => buf,
    }
}
