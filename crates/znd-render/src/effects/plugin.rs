//! 外部プラグイン（動的ライブラリ）によるエフェクト。
//!
//! C ABI の最小インターフェース:
//!
//! ```c
//! typedef struct { uint32_t abi_version; const char *name; } ZndPluginInfo;
//! const ZndPluginInfo *znd_plugin_info(void);
//! // rgba: リニア・プリマルチプライドの float RGBA（width*height*4 要素）をその場で書き換える
//! void znd_plugin_process(float *rgba, uint32_t width, uint32_t height,
//!                         double time_seconds, const double *params, uint32_t param_count);
//! ```
//!
//! OpenFX そのものではない（docs/01-architecture.md 参照）。プラグインはホストと同じ権限で動くので、
//! 信頼できるファイルだけを読み込むこと。

use super::FxContext;
use crate::frame::FrameBuf;
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::sync::{Mutex, OnceLock};
use znd_core::effect::EvalParams;

pub const PLUGIN_ABI_VERSION: u32 = 1;

#[repr(C)]
pub struct PluginInfo {
    pub abi_version: u32,
    pub name: *const c_char,
}

type InfoFn = unsafe extern "C" fn() -> *const PluginInfo;
type ProcessFn = unsafe extern "C" fn(*mut f32, u32, u32, f64, *const f64, u32);

struct Loaded {
    // ライブラリを保持し続ける（関数ポインタの寿命を保証する）
    _lib: libloading::Library,
    process: ProcessFn,
    name: String,
}

// 関数ポインタとライブラリハンドルはスレッド間で共有してよい（プラグイン側に再入可能であることを要求する）。
unsafe impl Send for Loaded {}
unsafe impl Sync for Loaded {}

fn cache() -> &'static Mutex<HashMap<String, Option<std::sync::Arc<Loaded>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<std::sync::Arc<Loaded>>>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn load(path: &str) -> Result<Loaded, String> {
    // SAFETY: 動的ライブラリの読み込みは初期化コードを実行する。ユーザーが明示的に指定したファイルに限る。
    unsafe {
        let lib = libloading::Library::new(path).map_err(|e| format!("{path}: 読み込めません: {e}"))?;
        let info_fn: libloading::Symbol<InfoFn> =
            lib.get(b"znd_plugin_info").map_err(|_| format!("{path}: znd_plugin_info がありません"))?;
        let info = info_fn();
        if info.is_null() {
            return Err(format!("{path}: プラグイン情報が取得できません"));
        }
        if (*info).abi_version != PLUGIN_ABI_VERSION {
            return Err(format!("{path}: ABI バージョン {} は未対応です（{PLUGIN_ABI_VERSION} が必要）", (*info).abi_version));
        }
        let name = if (*info).name.is_null() { String::new() } else { CStr::from_ptr((*info).name).to_string_lossy().into_owned() };
        let process: ProcessFn =
            *lib.get::<ProcessFn>(b"znd_plugin_process").map_err(|_| format!("{path}: znd_plugin_process がありません"))?;
        Ok(Loaded { _lib: lib, process, name })
    }
}

fn get(path: &str) -> Option<std::sync::Arc<Loaded>> {
    let mut c = cache().lock().unwrap();
    if let Some(hit) = c.get(path) {
        return hit.clone();
    }
    let loaded = match load(path) {
        Ok(l) => Some(std::sync::Arc::new(l)),
        Err(e) => {
            eprintln!("プラグイン: {e}");
            None
        }
    };
    c.insert(path.to_string(), loaded.clone());
    loaded
}

/// プラグインを読み込めるか確認し、名前を返す。
pub fn probe(path: &str) -> Result<String, String> {
    load(path).map(|l| l.name)
}

pub fn apply(p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    let path = p.s("path");
    if path.is_empty() {
        return buf;
    }
    let Some(plugin) = get(path) else { return buf };
    let params = [p.f("a"), p.f("b"), p.f("c"), p.f("d")];
    // SAFETY: バッファは width*height*4 要素あり、呼び出しの間は排他的に借用している。
    unsafe {
        (plugin.process)(
            buf.data.as_mut_ptr(),
            buf.width as u32,
            buf.height as u32,
            ctx.time.seconds(),
            params.as_ptr(),
            params.len() as u32,
        );
    }
    buf
}
