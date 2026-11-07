//! 異常終了の直前に、作業中のプロジェクトを自動保存へ退避する。
//!
//! 編集のたびに最新のプロジェクト（`Arc` なので複製は安い）と退避先を覚えておき、画面のスレッドでパニックが
//! 起きたら、終了する前に自動保存を 1 世代書く。次の起動で「自動保存からの復旧」が出る。
//! プレビューや書き出しのスレッドのパニックは捕まえて処理を続けるので、ここでは扱わない。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use znd_core::{io, Project};

type Latest = Mutex<Option<(Arc<Project>, PathBuf)>>;

fn latest() -> &'static Latest {
    static L: OnceLock<Latest> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// 最新の状態と退避先を覚える。
pub(crate) fn remember(project: &Arc<Project>, autosave_base: &Path) {
    let mut l = latest().lock().unwrap_or_else(|e| e.into_inner());
    *l = Some((project.clone(), autosave_base.to_path_buf()));
}

/// 覚えている状態を自動保存に書く。書いたファイルを返す。
pub(crate) fn save_now() -> Option<PathBuf> {
    // パニックの途中なので、ロックは待たない
    let l = latest().try_lock().ok()?;
    let (project, base) = l.as_ref()?;
    io::autosave(project, base, 20).ok()
}

/// パニック時の退避を仕掛ける。既存のフック（メッセージの表示）はそのまま呼ぶ。
pub(crate) fn install_panic_hook() {
    static DONE: AtomicBool = AtomicBool::new(false);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        let main = std::thread::current().name() == Some("main");
        if main && !DONE.swap(true, Ordering::SeqCst) {
            match save_now() {
                Some(p) => eprintln!("znd: 異常終了する前に、作業中のプロジェクトを退避しました: {}", p.display()),
                None => eprintln!("znd: 作業中のプロジェクトを退避できませんでした"),
            }
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescue_writes_latest_state_to_autosave() {
        let dir = std::env::temp_dir().join(format!("znd-rescue-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("work.znd.json");
        let mut p = znd_core::demo::demo_project();
        p.compositions[0].name = "締切前".into();
        remember(&Arc::new(p), &base);
        let saved = save_now().unwrap();
        let back = io::load(&saved).unwrap();
        assert_eq!(back.compositions[0].name, "締切前");
        // 保存済みファイルが無いので、次の起動で復旧の対象になる
        assert_eq!(io::newer_autosave(&base), Some(saved));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
