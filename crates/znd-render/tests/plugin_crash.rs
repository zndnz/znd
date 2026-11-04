//! プラグインの処理中にプロセスが異常終了したとき、そのプラグインが記録されるか（子プロセスで実際に落として確かめる）。

#![cfg(unix)]

use std::process::Command;
use znd_render::plugins::guard;

const KIND: &str = "plugin:test-crashy";

/// 子プロセスとして呼ばれたときだけ、プラグインの処理中に不正なメモリアクセスを起こす。
#[test]
#[ignore = "crash_is_recorded_for_next_launch から子プロセスとして実行する"]
fn crash_child() {
    if std::env::var_os("ZND_CRASH_CHILD").is_none() {
        return;
    }
    guard::install_crash_journal().unwrap();
    guard::call(KIND, || (), || {
        // SAFETY: わざと落とすためのテスト
        unsafe { std::ptr::null_mut::<u32>().write_volatile(1) };
    });
}

#[test]
fn crash_is_recorded_for_next_launch() {
    let dir = std::env::temp_dir().join(format!("znd-crash-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child", "--ignored", "--test-threads=1"])
        .env("ZND_CRASH_CHILD", "1")
        .env("ZND_CONFIG_DIR", &dir)
        .status()
        .unwrap();
    assert!(!status.success(), "子プロセスは異常終了するはず");
    let text = std::fs::read_to_string(dir.join("plugin-crash.txt")).unwrap();
    assert_eq!(text, KIND);
    // 次の起動で読み出すと消える（ZND_CONFIG_DIR はこのプロセスでは別のテストと共有しないよう、ここでだけ設定する）
    std::env::set_var("ZND_CONFIG_DIR", &dir);
    assert_eq!(guard::take_crash_record().as_deref(), Some(KIND));
    assert_eq!(guard::take_crash_record(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn normal_calls_leave_no_record() {
    // 処理中でなければ記録しない（シグナルハンドラは ACTIVE を見る）
    assert_eq!(guard::call("plugin:test-ok", || 0, || 7), 7);
}
