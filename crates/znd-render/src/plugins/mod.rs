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

pub mod native;
pub mod ofx;

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

pub(crate) trait Effect: Send + Sync {
    fn info(&self) -> &PluginInfo;
    fn apply(&self, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf;
}

#[derive(Default)]
struct Registry {
    effects: BTreeMap<String, Arc<dyn Effect>>,
    /// 読み込み済み（または失敗済み）のファイル。
    files: BTreeMap<PathBuf, Result<usize, String>>,
}

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

/// プラグインファイル（ネイティブのライブラリ、または `.ofx.bundle` / `.ofx`）を 1 つ読み込んで登録する。
/// 既に読み込み済みなら何もしない。
pub fn load_plugin_file(path: &Path) -> Result<Vec<PluginInfo>, String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(prev) = registry().lock().unwrap().files.get(&key) {
        return prev.clone().map(|_| Vec::new());
    }
    let result: Result<Vec<Arc<dyn Effect>>, String> = if is_ofx(path) {
        ofx::load(path)
    } else if is_native_lib(path) {
        native::load(path)
    } else {
        Err("プラグインとして読み込める形式ではありません（.dylib / .so / .dll / .ofx.bundle）".into())
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

/// フォルダの中のプラグインをすべて読み込む（フォルダ直下と、1 段下のフォルダまで）。
pub fn scan(dirs: &[PathBuf]) -> ScanReport {
    let mut report = ScanReport::default();
    let visit = |path: &Path, report: &mut ScanReport| {
        if !(is_ofx(path) || (is_native_lib(path) && path.is_file())) {
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

/// プラグインのエフェクトを適用する。読み込まれていなければ元の画像のまま返す。
pub fn apply(kind: &str, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let effect = registry().lock().unwrap().effects.get(kind).cloned();
    match effect {
        Some(e) => {
            let mut out = e.apply(p, buf, ctx);
            // アルファが 0..1 を外れると後段の合成が破綻するので、ここで収める
            out.data.chunks_exact_mut(4).for_each(|px| px[3] = px[3].clamp(0.0, 1.0));
            out
        }
        None => buf,
    }
}
