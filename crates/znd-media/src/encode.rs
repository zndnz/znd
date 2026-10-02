//! FFmpeg への動画エンコード。生フレームをパイプで流し込む。

use crate::{tools, MediaError};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use znd_core::FrameRate;

#[derive(Debug, Clone, PartialEq)]
pub struct H264Settings {
    /// 画質（小さいほど高画質）。上映用途を想定して既定は 14。
    pub crf: u8,
    /// x264 のプリセット名。
    pub preset: String,
}

impl Default for H264Settings {
    fn default() -> Self {
        H264Settings { crf: 14, preset: "slow".into() }
    }
}

/// パイプに流す生フレームの形式。
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PipeFormat {
    /// 8bit RGB（ディザ済み）。
    Rgb8,
    /// 16bit RGBA（ストレートアルファ、リトルエンディアン）。
    Rgba16,
}

impl PipeFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PipeFormat::Rgb8 => 3,
            PipeFormat::Rgba16 => 8,
        }
    }
    fn pix_fmt(self) -> &'static str {
        match self {
            PipeFormat::Rgb8 => "rgb24",
            PipeFormat::Rgba16 => "rgba64le",
        }
    }
}

pub struct VideoEncoder {
    child: Child,
    stdin: Option<ChildStdin>,
    frame_bytes: usize,
}

impl VideoEncoder {
    /// `output_args` は入力指定の後ろ・出力ファイルの前に置く引数（フィルター・コーデック等）。
    /// `audio` を渡すと 2 番目の入力として読み込む。
    pub fn spawn(
        input: PipeFormat,
        width: usize,
        height: usize,
        fps: FrameRate,
        audio: Option<&Path>,
        output_args: &[String],
        path: &Path,
    ) -> Result<Self, MediaError> {
        let mut cmd = Command::new(tools::ffmpeg()?);
        cmd.args(["-v", "error", "-nostats", "-y", "-f", "rawvideo", "-pix_fmt", input.pix_fmt()]);
        cmd.args(["-s", &format!("{width}x{height}"), "-r", &format!("{}/{}", fps.num, fps.den), "-i", "-"]);
        if let Some(a) = audio {
            cmd.arg("-i").arg(a);
        }
        let mut child = cmd
            .args(output_args)
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| MediaError::Tool { tool: "ffmpeg", message: e.to_string() })?;
        let stdin = child.stdin.take();
        Ok(VideoEncoder { child, stdin, frame_bytes: width * height * input.bytes_per_pixel() })
    }

    pub fn write_frame(&mut self, data: &[u8]) -> Result<(), MediaError> {
        assert_eq!(data.len(), self.frame_bytes, "frame size mismatch");
        let stdin = self.stdin.as_mut().expect("encoder already finished");
        if stdin.write_all(data).is_err() {
            // パイプが切れた = ffmpeg が異常終了した。stderr に理由がある。
            return Err(self.failure());
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), MediaError> {
        drop(self.stdin.take());
        let status = self.child.wait().map_err(|e| MediaError::Tool { tool: "ffmpeg", message: e.to_string() })?;
        if status.success() {
            Ok(())
        } else {
            Err(self.failure())
        }
    }

    fn failure(&mut self) -> MediaError {
        drop(self.stdin.take());
        let _ = self.child.wait();
        let mut message = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut message);
        }
        MediaError::Tool { tool: "ffmpeg", message: message.trim().to_string() }
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        // finish されずに破棄された（中断・エラー）場合にプロセスを残さない
        if self.stdin.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// ffmpeg を 1 回実行して終了を待つ（2 パスエンコードなど、パイプを使わない処理用）。
pub fn run_ffmpeg(args: &[String]) -> Result<(), MediaError> {
    let out = Command::new(tools::ffmpeg()?)
        .args(["-v", "error", "-nostats", "-nostdin", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| MediaError::Tool { tool: "ffmpeg", message: e.to_string() })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(MediaError::Tool { tool: "ffmpeg", message: String::from_utf8_lossy(&out.stderr).trim().to_string() })
    }
}
