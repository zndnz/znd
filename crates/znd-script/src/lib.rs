//! スクリプト層: エクスプレッション（プロパティ単位の JavaScript）と自動化スクリプト（プロジェクト全体の操作）。

pub mod automation;
pub mod expr;

pub use automation::{run_plugin, run_script, PluginRun};
pub use expr::{source_rect, ExprEngine, ExprError};
