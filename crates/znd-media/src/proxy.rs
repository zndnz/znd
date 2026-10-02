//! プロキシ（低解像度の代替ファイル）の生成。重い素材でもプレビューを軽くするため。

use crate::footage::resolve_path;
use crate::{tools, MediaError};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use znd_core::FootageItem;

pub const PROXY_DIR: &str = ".znd-proxy";

pub fn proxy_path(base_dir: &Path, item: &FootageItem) -> PathBuf {
    base_dir.join(PROXY_DIR).join(format!("{}.mp4", item.id))
}

/// 1/`divisor` 解像度・全フレームがキーフレームのプロキシを作る（どのフレームへも素早くシークできる）。
/// 生成したファイルのパスと解像度を返す。
pub fn make_proxy(base_dir: &Path, item: &FootageItem, divisor: u32) -> Result<(PathBuf, u32, u32), MediaError> {
    let Some(fps) = item.frame_rate else {
        return Err(MediaError::Invalid(format!("{}: 動画ではないのでプロキシは不要です", item.name)));
    };
    let src = resolve_path(base_dir, &item.path);
    let out = proxy_path(base_dir, item);
    std::fs::create_dir_all(out.parent().unwrap())
        .map_err(|e| MediaError::File { path: out.display().to_string(), message: e.to_string() })?;
    // yuv420p は偶数寸法が必要
    let d = divisor.max(1);
    let (w, h) = (((item.width / d).max(2) / 2) * 2, ((item.height / d).max(2) / 2) * 2);
    let mut cmd = Command::new(tools::ffmpeg()?);
    cmd.args(["-v", "error", "-nostdin", "-y", "-i"]).arg(&src);
    let mut vf = format!("scale={w}:{h}:flags=area");
    if item.vfr {
        // 可変フレームレート素材は、プロキシの時点で固定レートにしておく
        vf = format!("fps=fps={}/{}:round=near,{vf}", fps.num, fps.den);
    }
    let status = cmd
        .args([
            "-map", "0:v:0", "-an", "-vf", &vf, "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-g", "1", "-pix_fmt", "yuv420p",
        ])
        .arg(&out)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| MediaError::Tool { tool: "ffmpeg", message: e.to_string() })?;
    if !status.status.success() {
        return Err(MediaError::Tool { tool: "ffmpeg", message: String::from_utf8_lossy(&status.stderr).trim().to_string() });
    }
    Ok((out, w, h))
}
