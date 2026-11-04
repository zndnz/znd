//! プロジェクトファイルの保存・読み込み・自動保存。

use crate::model::{Project, FORMAT_VERSION};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: プロジェクトファイルを解釈できません: {source}")]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error("{path}: 形式バージョン {found} は未対応です（このビルドは {supported} まで）")]
    Version { path: PathBuf, found: u32, supported: u32 },
}

pub const AUTOSAVE_DIR: &str = ".znd-autosave";

pub fn to_json(project: &Project) -> String {
    let mut s = serde_json::to_string_pretty(project).expect("project is always serializable");
    s.push('\n');
    s
}

pub fn load(path: &Path) -> Result<Project, IoError> {
    let io = |source| IoError::Io { path: path.to_path_buf(), source };
    let text = fs::read_to_string(path).map_err(io)?;
    let project: Project = serde_json::from_str(&text).map_err(|source| IoError::Parse { path: path.to_path_buf(), source })?;
    if project.format_version > FORMAT_VERSION {
        return Err(IoError::Version { path: path.to_path_buf(), found: project.format_version, supported: FORMAT_VERSION });
    }
    Ok(project)
}

/// 一時ファイルに書いてから rename する。書き込み途中でクラッシュしても既存ファイルは壊れない。
pub fn save(project: &Project, path: &Path) -> Result<(), IoError> {
    let io = |source| IoError::Io { path: path.to_path_buf(), source };
    let tmp = path.with_extension("tmp-save");
    {
        let mut f = fs::File::create(&tmp).map_err(io)?;
        f.write_all(to_json(project).as_bytes()).map_err(io)?;
        f.sync_all().map_err(io)?;
    }
    fs::rename(&tmp, path).map_err(io)
}

fn autosave_dir(project_path: &Path) -> PathBuf {
    project_path.parent().unwrap_or(Path::new(".")).join(AUTOSAVE_DIR)
}

fn stem(project_path: &Path) -> String {
    project_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into())
}

/// 自動保存を 1 世代書き、古い世代を `keep` 個まで残して削除する。
pub fn autosave(project: &Project, project_path: &Path, keep: usize) -> Result<PathBuf, IoError> {
    let dir = autosave_dir(project_path);
    fs::create_dir_all(&dir).map_err(|source| IoError::Io { path: dir.clone(), source })?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("{}.{secs}.json", stem(project_path)));
    save(project, &path)?;
    for old in list_autosaves(project_path).into_iter().skip(keep) {
        let _ = fs::remove_file(old);
    }
    Ok(path)
}

/// 自動保存ファイルを新しい順に返す。
pub fn list_autosaves(project_path: &Path) -> Vec<PathBuf> {
    let prefix = format!("{}.", stem(project_path));
    let Ok(rd) = fs::read_dir(autosave_dir(project_path)) else {
        return Vec::new();
    };
    let mut found: Vec<(u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let secs = name.strip_prefix(&prefix)?.strip_suffix(".json")?.parse().ok()?;
            Some((secs, e.path()))
        })
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, p)| p).collect()
}

pub const SNAPSHOT_DIR: &str = ".znd-snapshots";

fn snapshot_dir(project_path: &Path) -> PathBuf {
    project_path.parent().unwrap_or(Path::new(".")).join(SNAPSHOT_DIR).join(stem(project_path))
}

/// 版（スナップショット）を保存する。「1次提出版」「最終版」など、名前を付けて残す。
pub fn save_snapshot(project: &Project, project_path: &Path, name: &str) -> Result<PathBuf, IoError> {
    let dir = snapshot_dir(project_path);
    fs::create_dir_all(&dir).map_err(|source| IoError::Io { path: dir.clone(), source })?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // ファイル名に使えない文字を避ける
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let path = dir.join(format!("{secs}-{safe}.json"));
    save(project, &path)?;
    Ok(path)
}

/// 保存済みの版を古い順に (名前, UNIX 秒, パス) で返す。
pub fn list_snapshots(project_path: &Path) -> Vec<(String, u64, PathBuf)> {
    let Ok(rd) = fs::read_dir(snapshot_dir(project_path)) else { return Vec::new() };
    let mut out: Vec<(String, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let (secs, name) = file.strip_suffix(".json")?.split_once('-')?;
            Some((name.to_string(), secs.parse().ok()?, e.path()))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}

/// プロジェクトファイルより新しい自動保存があればそのパスを返す（クラッシュ復旧用）。
pub fn newer_autosave(project_path: &Path) -> Option<PathBuf> {
    let newest = list_autosaves(project_path).into_iter().next()?;
    let auto_time = fs::metadata(&newest).ok()?.modified().ok()?;
    match fs::metadata(project_path).and_then(|m| m.modified()) {
        Ok(saved) if saved >= auto_time => None,
        _ => Some(newest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_project;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("znd-io-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn json_roundtrip_is_lossless_and_stable() {
        let p = demo_project();
        let json = to_json(&p);
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        assert_eq!(json, to_json(&back));
    }

    #[test]
    fn interrupted_save_and_broken_files_do_not_lose_work() {
        let dir = temp_dir("broken");
        let path = dir.join("p.znd.json");
        let mut p = demo_project();
        save(&p, &path).unwrap();
        // 書き込み途中で落ちた: 一時ファイルだけが中途半端に残る。元のファイルは無傷
        fs::write(path.with_extension("tmp-save"), "{\"format_version\": 1, \"compo").unwrap();
        assert_eq!(load(&path).unwrap(), p);
        // 次の保存は残骸を上書きして成功する
        p.compositions[0].name = "2 次提出".into();
        save(&p, &path).unwrap();
        assert_eq!(load(&path).unwrap().compositions[0].name, "2 次提出");
        // 途中で切れたファイルは、パニックせずにエラーになる
        let text = to_json(&p);
        let cut = dir.join("cut.znd.json");
        fs::write(&cut, &text[..text.len() / 2]).unwrap();
        assert!(matches!(load(&cut), Err(IoError::Parse { .. })));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_load_and_version_check() {
        let dir = temp_dir("save");
        let path = dir.join("p.znd.json");
        let mut p = demo_project();
        save(&p, &path).unwrap();
        assert_eq!(load(&path).unwrap(), p);
        p.format_version = FORMAT_VERSION + 1;
        save(&p, &path).unwrap();
        assert!(matches!(load(&path), Err(IoError::Version { .. })));
        fs::write(&path, "{ broken").unwrap();
        assert!(matches!(load(&path), Err(IoError::Parse { .. })));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshots_are_named_and_listed_in_order() {
        let dir = temp_dir("snap");
        let path = dir.join("作品.znd.json");
        let mut p = demo_project();
        assert!(list_snapshots(&path).is_empty());
        let first = save_snapshot(&p, &path, "1次提出版").unwrap();
        p.name = "変更後".into();
        // 同じ秒に保存しても名前が違えば別ファイル
        let second = save_snapshot(&p, &path, "最終/版").unwrap();
        let list = list_snapshots(&path);
        assert_eq!(list.len(), 2);
        let names: Vec<&str> = list.iter().map(|s| s.0.as_str()).collect();
        assert!(names.contains(&"1次提出版") && names.contains(&"最終_版"), "{names:?}");
        assert_eq!(load(&first).unwrap().name, "デモ");
        assert_eq!(load(&second).unwrap().name, "変更後");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn autosave_rotates_and_detects_recovery() {
        let dir = temp_dir("auto");
        let path = dir.join("p.znd.json");
        let p = demo_project();
        // 未保存プロジェクトでも自動保存があれば復旧候補になる
        let a = autosave(&p, &path, 3).unwrap();
        assert_eq!(newer_autosave(&path), Some(a.clone()));
        assert_eq!(load(&a).unwrap(), p);
        // 世代数の上限
        for secs in 1..=5 {
            fs::write(dir.join(AUTOSAVE_DIR).join(format!("p.znd.{secs}.json")), "{}").unwrap();
        }
        autosave(&p, &path, 3).unwrap();
        assert_eq!(list_autosaves(&path).len(), 3);
        fs::remove_dir_all(dir).unwrap();
    }
}
