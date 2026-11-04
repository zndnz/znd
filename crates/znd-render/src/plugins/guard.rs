//! プラグインの不具合からホストを守る仕組み。
//!
//! - **パニックの隔離**: プラグインの処理がパニックしたら、そのエフェクトをこのセッションの間止め（元の画像を通す）、
//!   理由を [`faults`] に残す。レンダリングと書き出しは止まらない。
//! - **異常終了の記録**: プラグインはホストと同じプロセスで動くので、ネイティブコードの不正なメモリアクセスなどは
//!   捕まえられない。そこで、プラグインの処理中に致命的なシグナル（SIGSEGV など）を受けたら、そのプラグインの
//!   種類名を記録ファイルに書いてから、元の処理（通常のクラッシュ）に戻す。次の起動時に [`take_crash_record`] で
//!   読み出して、利用者に「無効にするか」を尋ねる。Unix のみ。
//! - **無効化の一覧**: 利用者が無効にしたプラグインは設定フォルダの `plugin-disabled.json` に残し、読み込んでも適用しない。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// プラグインの処理がパニック・失敗した記録（種類名, 理由）。
fn fault_list() -> &'static Mutex<Vec<(String, String)>> {
    static F: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

/// このセッションの間止めているエフェクト。
fn quarantine() -> &'static Mutex<BTreeSet<String>> {
    static Q: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    Q.get_or_init(Default::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // 他のスレッドがパニックしても、記録は読み書きできるようにする
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// これまでに起きたプラグインの不具合（種類名, 理由）。
pub fn faults() -> Vec<(String, String)> {
    lock(fault_list()).clone()
}

pub fn is_quarantined(kind: &str) -> bool {
    lock(quarantine()).contains(kind)
}

/// 止めたエフェクトをもう一度使えるようにする。
pub fn release(kind: &str) {
    lock(quarantine()).remove(kind);
}

fn panic_message(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "不明なエラー".into())
}

// ---------- 異常終了の記録 ----------

/// 処理中のプラグインの数と、最後に処理に入ったプラグインの種類名（NUL 終端・解放しない）。
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static CURRENT: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

/// 種類名ごとに 1 回だけ確保した、NUL 終端の文字列（シグナルハンドラから読むので解放しない）。
fn leaked_name(kind: &str) -> *mut u8 {
    static NAMES: OnceLock<Mutex<std::collections::HashMap<String, usize>>> = OnceLock::new();
    let mut names = lock(NAMES.get_or_init(Default::default));
    let addr = *names.entry(kind.to_string()).or_insert_with(|| {
        let mut bytes = kind.as_bytes().to_vec();
        bytes.retain(|b| *b != 0);
        bytes.push(0);
        Box::leak(bytes.into_boxed_slice()).as_mut_ptr() as usize
    });
    addr as *mut u8
}

struct Active;

impl Active {
    fn enter(kind: &str) -> Active {
        CURRENT.store(leaked_name(kind), Ordering::SeqCst);
        ACTIVE.fetch_add(1, Ordering::SeqCst);
        Active
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// プラグインの処理 `f` を守って実行する。止めているエフェクトなら `fallback` を返す。
/// パニックしたらエフェクトを止めて記録し、`fallback` を返す。
pub fn call<T>(kind: &str, fallback: impl FnOnce() -> T, f: impl FnOnce() -> T) -> T {
    if is_quarantined(kind) || is_disabled(kind) {
        return fallback();
    }
    let r = {
        let _active = Active::enter(kind);
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
    };
    match r {
        Ok(v) => v,
        Err(e) => {
            let msg = panic_message(&*e);
            lock(quarantine()).insert(kind.to_string());
            let mut list = lock(fault_list());
            if !list.iter().any(|(k, _)| k == kind) {
                list.push((kind.to_string(), format!("処理中にエラーが起きたため、このセッションの間は止めました: {msg}")));
            }
            fallback()
        }
    }
}

/// 異常終了の記録ファイル。`ZND_CONFIG_DIR` があればその中。
pub fn crash_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("plugin-crash.txt"))
}

fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("ZND_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    super::trust::trust_file().and_then(|f| f.parent().map(|p| p.to_path_buf()))
}

/// 前回の起動でプラグインの処理中に異常終了していれば、その種類名を返して記録を消す。
pub fn take_crash_record() -> Option<String> {
    let path = crash_file()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::write(&path, "");
    let kind = text.trim().trim_matches('\0').to_string();
    (!kind.is_empty()).then_some(kind)
}

#[cfg(unix)]
mod signals {
    use super::{ACTIVE, CURRENT};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::OnceLock;

    static FD: AtomicI32 = AtomicI32::new(-1);
    const SIGNALS: [libc::c_int; 5] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGABRT];

    fn old_actions() -> &'static [std::sync::Mutex<Option<libc::sigaction>>; 5] {
        static OLD: OnceLock<[std::sync::Mutex<Option<libc::sigaction>>; 5]> = OnceLock::new();
        OLD.get_or_init(Default::default)
    }

    /// シグナルハンドラ。非同期シグナル安全な関数（ftruncate / lseek / write / sigaction）だけを使う。
    extern "C" fn on_fatal(sig: libc::c_int, _info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
        let fd = FD.load(Ordering::SeqCst);
        let name = CURRENT.load(Ordering::SeqCst);
        if fd >= 0 && !name.is_null() && ACTIVE.load(Ordering::SeqCst) > 0 {
            // SAFETY: name は解放しない NUL 終端の文字列。fd は起動時に開いたまま
            unsafe {
                let len = libc::strlen(name as *const libc::c_char);
                libc::ftruncate(fd, 0);
                libc::lseek(fd, 0, libc::SEEK_SET);
                libc::write(fd, name as *const libc::c_void, len);
            }
        }
        // 元のハンドラに戻して返る。不正アクセスは同じ命令でもう一度起き、元の処理（Rust のスタックあふれの
        // 表示や、通常のクラッシュ）に渡る。abort は自分で既定の処理に戻して送り直す
        if let Some(i) = SIGNALS.iter().position(|s| *s == sig) {
            // ハンドラの中ではロックを待たない
            if let Ok(old) = old_actions()[i].try_lock() {
                // SAFETY: 以前の設定を戻すだけ
                unsafe {
                    match &*old {
                        Some(a) => libc::sigaction(sig, a, std::ptr::null_mut()),
                        None => libc::signal(sig, libc::SIG_DFL) as libc::c_int,
                    };
                }
                return;
            }
        }
        // SAFETY: 既定の処理に戻す
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
        }
    }

    pub fn install(path: &std::path::Path) -> Result<(), String> {
        if FD.load(Ordering::SeqCst) >= 0 {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: 普通のファイルを開くだけ。前回の記録は消さない（読む前に消えないよう、O_TRUNC は付けない）
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_CLOEXEC, 0o644) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        FD.store(fd, Ordering::SeqCst);
        for (i, sig) in SIGNALS.iter().enumerate() {
            // SAFETY: sigaction 構造体を 0 で初期化してから必要な項目を埋める
            unsafe {
                let mut act: libc::sigaction = std::mem::zeroed();
                let handler: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) = on_fatal;
                act.sa_sigaction = handler as usize;
                act.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut act.sa_mask);
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(*sig, &act, &mut old) == 0 {
                    *old_actions()[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(old);
                }
            }
        }
        Ok(())
    }
}

/// 異常終了の記録を始める（起動時に 1 回呼ぶ。前回の記録は先に [`take_crash_record`] で読んでおくこと）。
pub fn install_crash_journal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let path = crash_file().ok_or("設定フォルダが分かりません")?;
        signals::install(&path)
    }
    #[cfg(not(unix))]
    {
        Ok(())
    }
}

// ---------- 無効化の一覧 ----------

fn disabled_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("plugin-disabled.json"))
}

fn disabled_set() -> &'static Mutex<BTreeSet<String>> {
    static D: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    D.get_or_init(|| {
        let set = disabled_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default();
        Mutex::new(set.into_iter().collect())
    })
}

/// 利用者が無効にしたプラグイン（種類名）。
pub fn disabled() -> Vec<String> {
    lock(disabled_set()).iter().cloned().collect()
}

pub fn is_disabled(kind: &str) -> bool {
    lock(disabled_set()).contains(kind)
}

/// プラグインを無効／有効にして保存する。
pub fn set_disabled(kind: &str, off: bool) -> Result<(), String> {
    let list: Vec<String> = {
        let mut set = lock(disabled_set());
        if off {
            set.insert(kind.to_string());
        } else {
            set.remove(kind);
        }
        set.iter().cloned().collect()
    };
    let file = disabled_file().ok_or("設定フォルダが分かりません")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, serde_json::to_string_pretty(&list).unwrap_or_default()).map_err(|e| format!("{}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_is_contained_and_quarantined() {
        let kind = "plugin:test-guard-panic";
        let v = call(kind, || 1, || -> i32 { panic!("壊れたプラグイン") });
        assert_eq!(v, 1);
        assert!(is_quarantined(kind));
        assert!(faults().iter().any(|(k, m)| k == kind && m.contains("壊れたプラグイン")));
        // 止めている間は呼ばない
        let v = call(kind, || 2, || -> i32 { unreachable!() });
        assert_eq!(v, 2);
        release(kind);
        assert_eq!(call(kind, || 0, || 3), 3);
    }
}
