//! 音声のデコード（FFmpeg → 48kHz ステレオ f32）と WAV 書き出し。

use crate::footage::resolve_path;
use crate::{tools, MediaError};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use znd_audio::{Clip, ClipSource, SAMPLE_RATE};
use znd_core::FootageItem;

/// ファイルの音声をすべてデコードする。
pub fn decode_audio(path: &Path) -> Result<Clip, MediaError> {
    let out = Command::new(tools::ffmpeg()?)
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-f", "f32le", "-ac", "2", "-ar", &SAMPLE_RATE.to_string(), "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| MediaError::Tool { tool: "ffmpeg", message: e.to_string() })?;
    if !out.status.success() {
        return Err(MediaError::File {
            path: path.display().to_string(),
            message: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    let data = out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    Ok(Clip { data })
}

/// FFmpeg でデコードした音声を保持するクリップ供給元。
pub struct FfmpegClips {
    base_dir: PathBuf,
    cache: Mutex<HashMap<PathBuf, Arc<Clip>>>,
}

impl FfmpegClips {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        FfmpegClips { base_dir: base_dir.into(), cache: Mutex::default() }
    }
}

impl ClipSource for FfmpegClips {
    fn clip(&self, item: &FootageItem) -> Result<Arc<Clip>, String> {
        let path = resolve_path(&self.base_dir, &item.path);
        if let Some(c) = self.cache.lock().unwrap().get(&path) {
            return Ok(c.clone());
        }
        let clip = Arc::new(decode_audio(&path).map_err(|e| e.to_string())?);
        self.cache.lock().unwrap().insert(path, clip.clone());
        Ok(clip)
    }
}

/// 32bit float・ステレオの WAV を書く。
pub fn write_wav(path: &Path, stereo: &[f32]) -> Result<(), MediaError> {
    let err = |e: std::io::Error| MediaError::File { path: path.display().to_string(), message: e.to_string() };
    let data_len = (stereo.len() * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).map_err(err)?);
    let mut header = Vec::with_capacity(58);
    header.extend(b"RIFF");
    header.extend((4 + 26 + 12 + 8 + data_len).to_le_bytes());
    header.extend(b"WAVEfmt ");
    header.extend(18u32.to_le_bytes());
    header.extend(3u16.to_le_bytes()); // IEEE float
    header.extend(2u16.to_le_bytes());
    header.extend(SAMPLE_RATE.to_le_bytes());
    header.extend((SAMPLE_RATE * 8).to_le_bytes());
    header.extend(8u16.to_le_bytes());
    header.extend(32u16.to_le_bytes());
    header.extend(0u16.to_le_bytes());
    // float 形式には fact チャンクが必要
    header.extend(b"fact");
    header.extend(4u32.to_le_bytes());
    header.extend((stereo.len() as u32 / 2).to_le_bytes());
    header.extend(b"data");
    header.extend(data_len.to_le_bytes());
    f.write_all(&header).map_err(err)?;
    for s in stereo {
        f.write_all(&s.to_le_bytes()).map_err(err)?;
    }
    f.flush().map_err(err)
}
