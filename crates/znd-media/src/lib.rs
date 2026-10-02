//! メディア I/O。動画は FFmpeg を外部プロセスとして起動して入出力する
//! （リンクしないので FFmpeg/x264 のライセンスが本体に波及しない）。

pub mod audio;
pub mod encode;
pub mod export;
pub mod footage;
pub mod import;
pub mod manage;
pub mod probe;
pub mod proxy;
pub mod psd;
pub mod review;
pub mod sequence;
pub mod tools;
pub mod vector;

pub use audio::{decode_audio, write_wav, FfmpegClips};
pub use encode::{H264Settings, VideoEncoder};
pub use export::{
    export, export_h264, notify, run_queue, save_png, ExportContext, ExportError, ExportReport, ExportSettings, Format, QueueEvent,
    QueueJob,
};
pub use footage::FfmpegFootage;
pub use import::{abc_camera_layer, import_file, ImportOptions, ImportOutcome};
pub use probe::{import_footage, import_footage_with_warnings, import_sequence, probe, MediaInfo};
pub use proxy::make_proxy;
pub use psd::{import_psd, parse_psd};
pub use vector::{import_vector, is_vector, VectorImport};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0} が見つかりません。FFmpeg をインストールするか、環境変数 ZND_FFMPEG_DIR で場所を指定してください")]
    ToolNotFound(&'static str),
    #[error("{tool} の実行に失敗しました: {message}")]
    Tool { tool: &'static str, message: String },
    #[error("{path}: {message}")]
    File { path: String, message: String },
    #[error("{0}")]
    Invalid(String),
}
