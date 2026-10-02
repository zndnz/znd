//! 連番画像。フッテージのパスに `frame_%04d.png` のような書式を持たせて 1 本の動画として扱う。

use std::path::{Path, PathBuf};
use znd_core::FrameRate;

/// 連番にはフレームレートが記録されていないので、読み込み時はこの値にする（あとから変更できる）。
pub const DEFAULT_RATE: FrameRate = FrameRate { num: 24, den: 1 };

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Sequence {
    /// 最初の番号。
    pub start: i64,
    /// 最初から最後までのコマ数（抜けを含む）。
    pub count: i64,
    pub missing: i64,
}

/// ファイル名の中の `%d` / `%0Nd` の位置と桁数。
fn placeholder(name: &str) -> Option<(usize, usize, usize)> {
    let start = name.find('%')?;
    let rest = &name[start + 1..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !rest[digits.len()..].starts_with('d') {
        return None;
    }
    let width = if digits.is_empty() { 0 } else { digits.trim_start_matches('0').parse().unwrap_or(0) };
    Some((start, start + 1 + digits.len() + 1, width))
}

pub fn is_pattern(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| placeholder(n).is_some())
}

/// 番号 `n` のコマのファイル。
pub fn frame_path(pattern: &Path, n: i64) -> PathBuf {
    let name = pattern.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    match placeholder(name) {
        Some((a, b, width)) => pattern.with_file_name(format!("{}{:0width$}{}", &name[..a], n, &name[b..], width = width)),
        None => pattern.to_path_buf(),
    }
}

/// 書式に一致するファイルを探して、番号の範囲を返す。
pub fn scan(pattern: &Path) -> Result<Sequence, String> {
    let name = pattern.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    let (a, b, width) = placeholder(name).ok_or_else(|| format!("{}: 連番の書式（%04d など）がありません", pattern.display()))?;
    let (prefix, suffix) = (&name[..a], &name[b..]);
    let dir = pattern.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut numbers: Vec<i64> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let f = e.file_name();
            let digits = f.to_str()?.strip_prefix(prefix)?.strip_suffix(suffix)?;
            let ok = !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) && (width == 0 || digits.len() == width);
            if ok {
                digits.parse().ok()
            } else {
                None
            }
        })
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    let (Some(&start), Some(&end)) = (numbers.first(), numbers.last()) else {
        return Err(format!("{}: 連番のファイルが見つかりません", pattern.display()));
    };
    let count = end - start + 1;
    Ok(Sequence { start, count, missing: count - numbers.len() as i64 })
}

/// 連番の 1 枚（`shot_0012.png`）から書式（`shot_%04d.png`）を作る。
/// 同じ規則のファイルが他に無ければ `None`（ただの静止画）。
pub fn pattern_from_file(file: &Path) -> Option<PathBuf> {
    let name = file.file_name()?.to_str()?;
    let stem_end = name.rfind('.').unwrap_or(name.len());
    let stem = &name[..stem_end];
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let prefix = &stem[..stem.len() - digits];
    let pattern = file.with_file_name(format!("{prefix}%0{digits}d{}", &name[stem_end..]));
    (scan(&pattern).ok()?.count > 1).then_some(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_formats_and_detects() {
        let p = Path::new("/a/shot_%04d.png");
        assert!(is_pattern(p));
        assert!(!is_pattern(Path::new("/a/100%.png")));
        assert_eq!(frame_path(p, 12), Path::new("/a/shot_0012.png"));
        assert_eq!(frame_path(Path::new("f%d.tga"), 7), Path::new("f7.tga"));
    }

    #[test]
    fn scan_finds_range_and_gaps() {
        let dir = std::env::temp_dir().join(format!("znd-seq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in [3, 4, 6, 7] {
            std::fs::write(dir.join(format!("s_{n:03}.png")), b"").unwrap();
        }
        std::fs::write(dir.join("s_0001.png"), b"").unwrap(); // 桁数が違うので別物
        let pattern = pattern_from_file(&dir.join("s_004.png")).unwrap();
        assert_eq!(pattern.file_name().unwrap(), "s_%03d.png");
        assert_eq!(scan(&pattern).unwrap(), Sequence { start: 3, count: 5, missing: 1 });
        assert!(pattern_from_file(&dir.join("s_0001.png")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
