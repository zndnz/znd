//! プラグインパッケージ（`plugin.json` を持つフォルダ）。
//!
//! エフェクトだけでなく、メニューのコマンド・パネル・ショートカット・ツールバーのボタン・
//! 出力フィルター（レンダリングへの介入）・エクスプレッション用ライブラリをまとめて提供できる。
//! スクリプト（JavaScript）は znd のスクリプト環境で動き、プロジェクトを読み書きできるが、
//! ファイル・ネットワーク・別プログラムには触れない。ネイティブのライブラリは制限なく動く。
//!
//! 読み込む前にフォルダ全体を安全性スキャンにかけ、危険度が高いものは利用者が許可するまで読み込まない。
//! スクリプトはスキャンした時点の中身をメモリに持ち、後からファイルが書き換えられても使わない。

use super::hooks::{self, FilterEffect, OutputFilter, Stage};
use super::scan::{self, Declared, SecurityReport};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub const MANIFEST: &str = "plugin.json";

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub description: String,
    /// 必要とする権限の宣言（"network" / "process" / "filesystem"）。スキャン結果と突き合わせる。
    #[serde(default)]
    pub permissions: Vec<String>,
    /// エフェクトのライブラリ（.dylib / .so / .dll / .ofx.bundle）。パッケージ内の相対パス。
    #[serde(default)]
    pub effects: Vec<String>,
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    #[serde(default)]
    pub panels: Vec<PanelDecl>,
    #[serde(default)]
    pub output_filters: Vec<FilterDecl>,
    /// エクスプレッションから呼べる関数を定義する JavaScript。
    #[serde(default)]
    pub expressions: Vec<String>,
    /// 書き出し形式（FFmpeg の引数の組）。
    #[serde(default)]
    pub exporters: Vec<ExporterDecl>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ExporterDecl {
    pub id: String,
    pub label: String,
    pub extension: String,
    #[serde(default)]
    pub video_args: Vec<String>,
    #[serde(default)]
    pub audio_args: Vec<String>,
    #[serde(default)]
    pub alpha: bool,
    #[serde(default)]
    pub even: bool,
    #[serde(default)]
    pub audio_only: bool,
    #[serde(default)]
    pub single_frame: bool,
    #[serde(default)]
    pub params: Vec<hooks::ExporterParam>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CommandDecl {
    pub id: String,
    pub label: String,
    pub script: String,
    /// 出す場所: "plugins"（プラグインメニュー。既定）/ "layer"（レイヤーの右クリック）/ "toolbar"。
    #[serde(default)]
    pub menu: Vec<String>,
    /// 例: "Cmd+Shift+K"、"Alt+F2"。
    #[serde(default)]
    pub shortcut: Option<String>,
    #[serde(default)]
    pub description: String,
    /// スクリプト内で呼ぶ関数。省略時はファイル全体を実行するだけ。
    #[serde(default)]
    pub function: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PanelDecl {
    pub id: String,
    pub title: String,
    pub script: String,
    #[serde(default)]
    pub width: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FilterDecl {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub enabled: bool,
    pub effects: Vec<FilterEffectDecl>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FilterEffectDecl {
    pub kind: String,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// `パッケージID/コマンドID`
    pub key: String,
    pub plugin: String,
    pub plugin_name: String,
    pub label: String,
    pub menu: Vec<String>,
    pub shortcut: Option<String>,
    pub description: String,
    pub function: Option<String>,
    pub source: Arc<String>,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub key: String,
    pub plugin: String,
    pub title: String,
    pub width: f32,
    pub source: Arc<String>,
    pub dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageStatus {
    Loaded,
    /// 危険度が高いため止めている（利用者の許可待ち）。
    Blocked,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub report: SecurityReport,
    pub status: PackageStatus,
    pub commands: Vec<Command>,
    pub panels: Vec<Panel>,
    /// 読み込んだエフェクトの数。
    pub effect_count: usize,
    /// 個別の読み込みの失敗（エフェクトのライブラリ・スクリプト）。
    pub warnings: Vec<String>,
}

pub fn is_package(path: &Path) -> bool {
    path.is_dir() && path.join(MANIFEST).is_file()
}

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("{MANIFEST} を読めません: {e}"))?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("{MANIFEST} の書式が不正です: {e}"))?;
    let valid = |s: &str| !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid(&m.id) {
        return Err(format!("パッケージの id「{}」が不正です（英数字と . _ - のみ）", m.id));
    }
    for id in m
        .commands
        .iter()
        .map(|c| &c.id)
        .chain(m.panels.iter().map(|p| &p.id))
        .chain(m.output_filters.iter().map(|f| &f.id))
        .chain(m.exporters.iter().map(|e| &e.id))
    {
        if !valid(id) {
            return Err(format!("id「{id}」が不正です（英数字と . _ - のみ）"));
        }
    }
    Ok(m)
}

/// パッケージ内の相対パスを解決する。外へ出るパス（`..`・絶対パス）は拒む。
pub fn resolve(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("「{rel}」: パッケージの外を指すパスは使えません"));
    }
    Ok(dir.join(p))
}

/// エフェクトのライブラリのパスを解決する。拡張子を省いた `lib/fx` は、OS に合わせて
/// `lib/fx.dylib` / `lib/libfx.dylib`（macOS）、`.so`（Linux）、`.dll`（Windows）を探す。
pub fn resolve_lib(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = resolve(dir, rel)?;
    if p.exists() {
        return Ok(p);
    }
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(windows) {
        "dll"
    } else {
        "so"
    };
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let candidates = [p.with_file_name(format!("{name}.{ext}")), p.with_file_name(format!("lib{name}.{ext}"))];
    Ok(candidates.iter().find(|c| c.exists()).cloned().unwrap_or(p))
}

pub fn declared(dir: &Path, m: &Manifest) -> Declared {
    Declared {
        manifest: true,
        permissions: m.permissions.iter().map(|s| s.to_ascii_lowercase()).collect::<BTreeSet<_>>(),
        binaries: m.effects.iter().filter_map(|e| resolve_lib(dir, e).ok()).collect(),
    }
}

/// スキャン済みのパッケージを有効にする（エフェクト・スクリプト・フィルター・ライブラリを登録）。
pub(crate) fn activate(dir: &Path, manifest: Manifest, report: SecurityReport) -> Package {
    let mut pkg = Package {
        dir: dir.to_path_buf(),
        report,
        status: PackageStatus::Loaded,
        commands: Vec::new(),
        panels: Vec::new(),
        effect_count: 0,
        warnings: Vec::new(),
        manifest,
    };
    let m = pkg.manifest.clone();
    for rel in &m.effects {
        match resolve_lib(dir, rel).and_then(|p| super::load_unchecked(&p)) {
            Ok(infos) => pkg.effect_count += infos.len(),
            Err(e) => pkg.warnings.push(format!("エフェクト {rel}: {e}")),
        }
    }
    let read = |rel: &str| -> Result<Arc<String>, String> {
        let p = resolve(dir, rel)?;
        std::fs::read_to_string(&p).map(Arc::new).map_err(|e| format!("{rel} を読めません: {e}"))
    };
    for c in &m.commands {
        match read(&c.script) {
            Ok(source) => pkg.commands.push(Command {
                key: format!("{}/{}", m.id, c.id),
                plugin: m.id.clone(),
                plugin_name: m.name.clone(),
                label: c.label.clone(),
                menu: if c.menu.is_empty() { vec!["plugins".into()] } else { c.menu.clone() },
                shortcut: c.shortcut.clone(),
                description: c.description.clone(),
                function: c.function.clone(),
                source,
                dir: dir.to_path_buf(),
            }),
            Err(e) => pkg.warnings.push(format!("コマンド {}: {e}", c.id)),
        }
    }
    for p in &m.panels {
        match read(&p.script) {
            Ok(source) => pkg.panels.push(Panel {
                key: format!("{}/{}", m.id, p.id),
                plugin: m.id.clone(),
                title: p.title.clone(),
                width: p.width.unwrap_or(280.0).clamp(160.0, 900.0),
                source,
                dir: dir.to_path_buf(),
            }),
            Err(e) => pkg.warnings.push(format!("パネル {}: {e}", p.id)),
        }
    }
    for f in &m.output_filters {
        hooks::register_filter(OutputFilter {
            key: format!("{}/{}", m.id, f.id),
            plugin: m.id.clone(),
            label: f.label.clone(),
            stage: Stage::parse(&f.stage),
            enabled: f.enabled,
            effects: f.effects.iter().map(|e| FilterEffect { kind: e.kind.clone(), params: e.params.clone() }).collect(),
        });
    }
    for e in &m.exporters {
        // 出力先を引数で指定させない（書き出し先はホストが決める）
        let bad = e.video_args.iter().chain(&e.audio_args).any(|a| a == "-i" || a == "-y" || a.starts_with("-filter_complex_script"));
        let ext_ok = !e.extension.is_empty() && e.extension.len() <= 16 && e.extension.chars().all(|c| c.is_ascii_alphanumeric());
        if bad || !ext_ok || (e.audio_only && e.audio_args.is_empty()) {
            pkg.warnings.push(format!("書き出し形式 {}: 引数か拡張子が不正です", e.id));
            continue;
        }
        hooks::register_exporter(hooks::Exporter {
            key: format!("{}/{}", m.id, e.id),
            plugin: m.id.clone(),
            label: e.label.clone(),
            extension: e.extension.to_ascii_lowercase(),
            video_args: e.video_args.clone(),
            audio_args: e.audio_args.clone(),
            alpha: e.alpha,
            even: e.even,
            audio_only: e.audio_only,
            single_frame: e.single_frame,
            params: e.params.clone(),
            description: e.description.clone(),
        });
    }
    for rel in &m.expressions {
        match read(rel) {
            Ok(src) => hooks::add_expression_library(&format!("{}/{rel}", m.id), &src),
            Err(e) => pkg.warnings.push(format!("エクスプレッション {rel}: {e}")),
        }
    }
    pkg
}

/// パッケージを調べて、問題なければ有効にする。危険度が高く許可されていなければ止める。
pub(crate) fn load(dir: &Path) -> Package {
    let manifest = match read_manifest(dir) {
        Ok(m) => m,
        Err(e) => {
            let report = scan::scan_path(dir, &Declared { manifest: true, ..Default::default() });
            return Package {
                manifest: Manifest {
                    id: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    version: String::new(),
                    vendor: String::new(),
                    license: String::new(),
                    description: String::new(),
                    permissions: Vec::new(),
                    effects: Vec::new(),
                    commands: Vec::new(),
                    panels: Vec::new(),
                    output_filters: Vec::new(),
                    expressions: Vec::new(),
                    exporters: Vec::new(),
                },
                dir: dir.to_path_buf(),
                report,
                status: PackageStatus::Failed(e),
                commands: Vec::new(),
                panels: Vec::new(),
                effect_count: 0,
                warnings: Vec::new(),
            };
        }
    };
    let report = scan::scan_path(dir, &declared(dir, &manifest));
    if report.level.blocks() && !super::trust::is_trusted(&report.hash) {
        return Package {
            manifest,
            dir: dir.to_path_buf(),
            report,
            status: PackageStatus::Blocked,
            commands: Vec::new(),
            panels: Vec::new(),
            effect_count: 0,
            warnings: Vec::new(),
        };
    }
    activate(dir, manifest, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escaping_paths() {
        let d = Path::new("/tmp/pkg");
        assert!(resolve(d, "../x.js").is_err());
        assert!(resolve(d, "/etc/passwd").is_err());
        assert_eq!(resolve(d, "scripts/a.js").unwrap(), d.join("scripts/a.js"));
    }

    #[test]
    fn loads_example_package() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/toolkit.zndplugin");
        let pkg = load(&dir);
        assert_eq!(pkg.status, PackageStatus::Loaded, "{:?}", pkg.report.findings);
        assert!(pkg.warnings.is_empty(), "{:?}", pkg.warnings);
        assert!(!pkg.commands.is_empty() && !pkg.panels.is_empty());
        assert_eq!(pkg.report.level, scan::RiskLevel::Safe, "{:#?}", pkg.report.findings);
        assert!(hooks::output_filters().iter().any(|f| f.plugin == pkg.manifest.id));
    }

    #[test]
    fn exporters_are_registered_and_libraries_resolve_without_extension() {
        let dir = std::env::temp_dir().join(format!("znd-pkg-exporters-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(
            dir.join(MANIFEST),
            r#"{ "id": "test.exp", "name": "書き出しテスト", "exporters": [
                { "id": "x264", "label": "x264", "extension": "mp4", "video_args": ["-c:v", "libx264", "-crf", "{crf}"],
                  "params": [{ "id": "crf", "label": "CRF", "default": 18 },
                             { "id": "preset", "label": "プリセット", "type": "choice", "default": 1,
                               "options": [{ "label": "速い", "value": "fast" }, "slow"] }] },
                { "id": "bad", "label": "不正", "extension": "mp4", "video_args": ["-i", "/etc/passwd"] }
            ] }"#,
        )
        .unwrap();
        let pkg = load(&dir);
        assert_eq!(pkg.status, PackageStatus::Loaded);
        assert_eq!(pkg.warnings.len(), 1, "{:?}", pkg.warnings);
        let ex = hooks::exporters().into_iter().find(|e| e.key == "test.exp/x264").unwrap();
        assert_eq!(ex.expand(&ex.video_args, &Default::default()), ["-c:v", "libx264", "-crf", "18"]);
        assert_eq!(ex.params[1].choices()[1], ("slow".to_string(), "slow".to_string()));
        assert_eq!(ex.params[1].default_value(), "slow");
        assert!(!hooks::exporters().iter().any(|e| e.key == "test.exp/bad"));
        // 拡張子を省いたライブラリは OS に合わせて探す
        let ext = if cfg!(target_os = "macos") { "dylib" } else if cfg!(windows) { "dll" } else { "so" };
        std::fs::write(dir.join(format!("lib/libfx.{ext}")), b"").unwrap();
        assert_eq!(resolve_lib(&dir, "lib/fx").unwrap(), dir.join(format!("lib/libfx.{ext}")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
