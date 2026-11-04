//! CPU リファレンスレンダラー。リニアライト・32bit float・プリマルチプライドで合成する。
//!
//! 参照画像テストの「正解」を定義する実装であり、GPU バックエンドはこの出力との
//! 差分で検証する（docs/01-architecture.md）。

pub mod alembic;
pub mod blend;
pub mod blur;
pub mod check;
pub mod color;
pub mod compositor;
pub mod effects;
pub mod fingerprint;
pub mod gpu_composite;
pub mod frame;
pub mod mask;
pub mod model;
pub mod output;
pub mod path;
pub mod plugins;
pub mod raster;
pub mod safety;
pub mod shape;
pub mod text;
pub mod three_d;

pub use color::{ColorXf, OutputXf};
pub use compositor::{AudioWindow, FootageSource, NoFootage, RenderError, RenderOptions, Renderer, Resolver};
pub use frame::{FrameBuf, Tile};
pub use output::{encode_rgb8, encode_rgba16, encode_rgba8, linear_output, to_rgb8, to_rgba8};
