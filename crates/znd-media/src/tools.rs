//! ffmpeg / ffprobe 実行ファイルの探索。

use crate::MediaError;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn find(name: &'static str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("ZND_FFMPEG_DIR") {
        candidates.push(PathBuf::from(dir).join(name));
    }
    candidates.push(PathBuf::from(name));
    // GUI から起動すると PATH に Homebrew 等が入っていないことがある
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        candidates.push(PathBuf::from(dir).join(name));
    }
    candidates.into_iter().find(|c| {
        Command::new(c).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    })
}

pub fn ffmpeg() -> Result<&'static PathBuf, MediaError> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| find("ffmpeg")).as_ref().ok_or(MediaError::ToolNotFound("ffmpeg"))
}

pub fn ffprobe() -> Result<&'static PathBuf, MediaError> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| find("ffprobe")).as_ref().ok_or(MediaError::ToolNotFound("ffprobe"))
}
