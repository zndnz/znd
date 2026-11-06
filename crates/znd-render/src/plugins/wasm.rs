//! WebAssembly のプラグイン（`.wasm`、znd WASM ABI 1）をサンドボックスで動かす。
//!
//! ネイティブのライブラリ（`.dylib` など）はホストと同じ権限で何でもできてしまうが、WebAssembly のモジュールは
//! 次の範囲でしか動けない:
//! - 自分の線形メモリ（上限 [`MEMORY_LIMIT`]）の中だけを読み書きする。ホストのメモリには触れない
//! - ホストが渡す関数（このファイルの `znd` モジュールの関数）だけを呼べる。WASI は渡さないので、
//!   ファイル・ネットワーク・別プログラム・環境変数・時計には一切触れない
//! - 1 回の呼び出しは [`CALL_TIMEOUT`]（[`set_call_timeout`] で変更可）で打ち切る（無限ループでアプリが固まらない）
//! - 不正な処理（範囲外アクセス・panic）はトラップになり、その回は元の画像を返してインスタンスを作り直す
//!
//! ホスト関数は、渡されたポインタと長さを必ずメモリの範囲と突き合わせてから読み書きする。
//!
//! ## ABI
//!
//! モジュールが書き出すもの:
//! - `memory`
//! - `znd_abi() -> i32`: 1
//! - `znd_describe() -> i64`: 定義の JSON の (ポインタ << 32) | 長さ
//! - `znd_alloc(len: i32) -> i32` / `znd_free(ptr: i32, len: i32)`: 8 バイト境界のメモリの確保と解放
//! - `znd_render(effect: i32, args: i32) -> i32`: 0 で成功
//! - `znd_process_audio(effect: i32, args: i32) -> i32`（音声エフェクトがあるとき）
//!
//! 描画の引数（`args` の指す先、リトルエンディアン）:
//! `rgba u32, width u32, height u32, param_count u32, time f64, fps f64, scale f64, layer_time f64,
//! layer_duration f64, seed u32, values u32, strings u32, _ u32`（72 バイト）。
//! `values` はパラメータごとに f64 × 4、`strings` はパラメータごとに (ポインタ u32, 長さ u32)。
//!
//! 音声の引数: `samples u32, frames u32, sample_rate u32, param_count u32, start i64, time f64,
//! instance u64, length i64, seed u32, values u32, strings u32, _ u32`（64 バイト）。

use super::native::{
    pack, valid_id, Param, EFFECT_AUDIO, EFFECT_READS_BELOW, EFFECT_THREAD_SAFE, EFFECT_TIME_DEPENDENT, MAX_SIDE, PARAM_BOOL,
    PARAM_CHOICE, PARAM_COLOR, PARAM_FLOAT, PARAM_LAYER, PARAM_PIXELS, PARAM_POINT, PARAM_TEXT,
};
use super::{Effect, PluginApi, PluginInfo};
use crate::effects::FxContext;
use crate::frame::FrameBuf;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use wasmtime::{Caller, Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};
use znd_core::effect::{register_audio_processor, register_effect, AudioBlock, AudioProcessor, DynEffect, DynPd, EvalParams};

pub const WASM_ABI: u32 = 1;
/// 1 インスタンスの線形メモリの上限。
pub const MEMORY_LIMIT: usize = 3 << 30;
/// 1 回の呼び出しの時間の上限（既定）。
pub const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
static TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(30_000);

/// 1 回の呼び出しの時間の上限を変える（検証用。重いエフェクトを書き出すときに延ばすのにも使える）。
pub fn set_call_timeout(d: std::time::Duration) {
    TIMEOUT_MS.store(d.as_millis().max(TICK.as_millis()) as u64, Ordering::Relaxed);
}
/// エポックを進める間隔。
const TICK: std::time::Duration = std::time::Duration::from_millis(10);
/// スレッドセーフなエフェクトのために同時に作るインスタンスの数の上限。
const POOL_MAX: usize = 4;
/// ホストがモジュールに渡す画像・音声の要素数の上限（誤った値で巨大な確保をしないため）。
const MAX_FLOATS: usize = 1 << 28;

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.epoch_interruption(true);
        config.cranelift_opt_level(wasmtime::OptLevel::Speed);
        let engine = Engine::new(&config).expect("WebAssembly の実行環境を作れません");
        // 呼び出しの時間切れを判定するための時計
        let e = engine.clone();
        let _ = std::thread::Builder::new().name("znd-wasm-epoch".into()).spawn(move || loop {
            std::thread::sleep(TICK);
            e.increment_epoch();
        });
        engine
    })
}

fn deadline_ticks() -> u64 {
    (TIMEOUT_MS.load(Ordering::Relaxed) / TICK.as_millis() as u64).max(1)
}

// ---------- モジュールのコンパイルとキャッシュ ----------

fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches/znd"))
    } else if let Some(x) = std::env::var_os("XDG_CACHE_HOME") {
        Some(PathBuf::from(x).join("znd"))
    } else if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        Some(PathBuf::from(l).join("znd"))
    } else {
        home.map(|h| h.join(".cache/znd"))
    };
    let engine = engine();
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut h);
    base.map(|b| b.join(format!("wasm-{:016x}", h.finish())))
}

fn modules() -> &'static Mutex<HashMap<PathBuf, Module>> {
    static M: OnceLock<Mutex<HashMap<PathBuf, Module>>> = OnceLock::new();
    M.get_or_init(Mutex::default)
}

/// `.wasm` をコンパイルする。同じ中身のコンパイル結果はキャッシュのフォルダから読む（次回の起動を速くする）。
fn compile(path: &Path) -> Result<Module, String> {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(m) = modules().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(m.clone());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("読めません: {e}"))?;
    if !bytes.starts_with(b"\0asm") {
        return Err("WebAssembly のモジュールではありません".into());
    }
    let mut hasher = super::scan::Sha256::new();
    hasher.update(&bytes);
    let cached = cache_dir().map(|d| d.join(format!("{}.cwasm", hasher.finish_hex())));
    let engine = engine();
    if let Some(c) = cached.as_ref().filter(|c| c.is_file()) {
        // SAFETY: キャッシュのファイルは、このプロセスと同じ利用者だけが書けるフォルダに、このエンジンで作ったもの
        // （エンジンの互換性のハッシュをフォルダ名に含めている）。読めなければ作り直す。
        if let Ok(m) = unsafe { Module::deserialize_file(engine, c) } {
            modules().lock().unwrap_or_else(|e| e.into_inner()).insert(key, m.clone());
            return Ok(m);
        }
    }
    let module = Module::new(engine, &bytes).map_err(|e| format!("WebAssembly をコンパイルできません: {e}"))?;
    if let (Some(c), Ok(ser)) = (&cached, module.serialize()) {
        if let Some(d) = c.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = c.with_extension("tmp");
        if std::fs::write(&tmp, ser).is_ok() {
            let _ = std::fs::rename(tmp, c);
        }
    }
    modules().lock().unwrap_or_else(|e| e.into_inner()).insert(key, module.clone());
    Ok(module)
}

/// まとめて先にコンパイルしておく（起動時に、パッケージを順に読み込む前に並列で）。
pub(crate) fn precompile(paths: &[PathBuf]) {
    use rayon::prelude::*;
    paths.par_iter().for_each(|p| {
        let _ = compile(p);
    });
}

// ---------- インスタンス ----------

/// 呼び出しの間だけ生きる、ホスト関数が使う状態。
struct Call<'a, 'b> {
    ctx: &'a FxContext<'b>,
    params: &'a EvalParams,
    names: &'a [Param],
    /// モジュールに読ませる画像（番号で指す）。
    images: RefCell<Vec<FrameBuf>>,
    /// `set_output` で受け取った出力と、その左上の位置。
    output: RefCell<Option<(FrameBuf, [i32; 2])>>,
    size: (usize, usize),
}

struct State {
    limits: StoreLimits,
    /// 描画中だけ `Call` を指す（それ以外は null）。
    call: *const Call<'static, 'static>,
    logs: usize,
}

// `call` は描画の呼び出しの間だけ、そのスレッドからだけ使う。
unsafe impl Send for State {}

struct Inst {
    store: Store<State>,
    instance: Instance,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<(u32, u32), ()>,
    render: TypedFunc<(u32, u32), i32>,
    audio: Option<TypedFunc<(u32, u32), i32>>,
}

fn new_store() -> Store<State> {
    let limits = StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).instances(1).tables(4).memories(1).trap_on_grow_failure(false).build();
    let mut store = Store::new(engine(), State { limits, call: std::ptr::null(), logs: 0 });
    store.limiter(|s| &mut s.limits);
    store.set_epoch_deadline(deadline_ticks());
    store
}

/// メモリの `ptr..ptr+len` を、範囲を確かめてから取り出す。
fn mem_slice(data: &[u8], ptr: u32, len: usize) -> Option<&[u8]> {
    data.get(ptr as usize..(ptr as usize).checked_add(len)?)
}

fn mem_slice_mut(data: &mut [u8], ptr: u32, len: usize) -> Option<&mut [u8]> {
    data.get_mut(ptr as usize..(ptr as usize).checked_add(len)?)
}

fn memory(caller: &mut Caller<'_, State>) -> Option<Memory> {
    caller.get_export("memory").and_then(|e| e.into_memory())
}

fn write_f32s(caller: &mut Caller<'_, State>, ptr: u32, values: &[f32]) -> bool {
    let Some(mem) = memory(caller) else { return false };
    match mem_slice_mut(mem.data_mut(caller), ptr, values.len() * 4) {
        Some(dst) => {
            for (d, v) in dst.chunks_exact_mut(4).zip(values) {
                d.copy_from_slice(&v.to_le_bytes());
            }
            true
        }
        None => false,
    }
}

fn read_f32s(caller: &mut Caller<'_, State>, ptr: u32, n: usize) -> Option<Vec<f32>> {
    if n > MAX_FLOATS {
        return None;
    }
    let mem = memory(caller)?;
    let src = mem_slice(mem.data(&*caller), ptr, n * 4)?;
    Some(src.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn read_str(caller: &mut Caller<'_, State>, ptr: u32, len: u32) -> Option<String> {
    if len > 1 << 20 {
        return None;
    }
    let mem = memory(caller)?;
    mem_slice(mem.data(&*caller), ptr, len as usize).map(|b| String::from_utf8_lossy(b).into_owned())
}

fn write_u32(caller: &mut Caller<'_, State>, ptr: u32, v: u32) -> bool {
    let Some(mem) = memory(caller) else { return false };
    match mem_slice_mut(mem.data_mut(caller), ptr, 4) {
        Some(d) => {
            d.copy_from_slice(&v.to_le_bytes());
            true
        }
        None => false,
    }
}

/// 描画中の状態。描画中でなければ None（describe や音声の処理から呼ばれた場合）。
fn call<'a>(caller: &Caller<'_, State>) -> Option<&'a Call<'a, 'a>> {
    let p = caller.data().call;
    // SAFETY: `call` は `WasmEffect::apply` の中で、そのスタック上の `Call` を指している間だけ非 null になる。
    (!p.is_null()).then(|| unsafe { &*p.cast::<Call<'a, 'a>>() })
}

fn hand_out(c: &Call, buf: FrameBuf) -> i32 {
    let mut imgs = c.images.borrow_mut();
    if imgs.len() >= 64 {
        return -1;
    }
    imgs.push(buf);
    imgs.len() as i32 - 1
}

fn layer_param(c: &Call, index: u32) -> Option<znd_core::LayerId> {
    let p = c.names.get(index as usize).filter(|p| p.kind == PARAM_LAYER)?;
    c.params.layer(&p.name).cloned()
}

fn linker() -> Result<Linker<State>, String> {
    let mut l: Linker<State> = Linker::new(engine());
    let e = |r: wasmtime::Result<&mut Linker<State>>| r.map(|_| ()).map_err(|e| e.to_string());
    e(l.func_wrap("znd", "frame_at", |caller: Caller<'_, State>, time: f64| -> i32 {
        let Some(c) = call(&caller) else { return -1 };
        if !time.is_finite() {
            return -1;
        }
        (c.ctx.fetch_self)(znd_core::Time::from_seconds(time)).map_or(-1, |b| hand_out(c, b))
    }))?;
    e(l.func_wrap("znd", "backdrop", |caller: Caller<'_, State>| -> i32 {
        let Some(c) = call(&caller) else { return -1 };
        (c.ctx.fetch_below)(c.size.0, c.size.1).map_or(-1, |b| hand_out(c, b))
    }))?;
    e(l.func_wrap("znd", "layer_image", |caller: Caller<'_, State>, param: u32| -> i32 {
        let Some(c) = call(&caller) else { return -1 };
        let Some(id) = layer_param(c, param) else { return -1 };
        (c.ctx.fetch_layer)(&id).map_or(-1, |b| hand_out(c, b))
    }))?;
    e(l.func_wrap("znd", "layer_source", |caller: Caller<'_, State>, param: u32| -> i32 {
        let Some(c) = call(&caller) else { return -1 };
        let Some(id) = layer_param(c, param) else { return -1 };
        (c.ctx.fetch_source)(&id).map_or(-1, |b| hand_out(c, b))
    }))?;
    e(l.func_wrap("znd", "image_size", |mut caller: Caller<'_, State>, handle: i32, w: u32, h: u32| -> i32 {
        let Some(c) = call(&caller) else { return 1 };
        let size = c.images.borrow().get(handle as usize).map(|b| (b.width as u32, b.height as u32));
        match size {
            Some((iw, ih)) if handle >= 0 => (!(write_u32(&mut caller, w, iw) && write_u32(&mut caller, h, ih))) as i32,
            _ => 1,
        }
    }))?;
    e(l.func_wrap("znd", "image_read", |mut caller: Caller<'_, State>, handle: i32, dst: u32, len: u32| -> i32 {
        let Some(c) = call(&caller) else { return 1 };
        if handle < 0 {
            return 1;
        }
        let imgs = c.images.borrow();
        let Some(img) = imgs.get(handle as usize) else { return 1 };
        if img.data.len() != len as usize {
            return 1;
        }
        let data = img.data.clone();
        drop(imgs);
        (!write_f32s(&mut caller, dst, &data)) as i32
    }))?;
    e(l.func_wrap("znd", "audio", |mut caller: Caller<'_, State>, param: u32, out: u32, len: u32| -> i32 {
        let Some(c) = call(&caller) else { return 1 };
        if len == 0 || len > 1 << 22 {
            return 1;
        }
        let Some(id) = layer_param(c, param) else { return 1 };
        let mut samples = (c.ctx.audio)(&id, len as usize);
        let empty = samples.is_empty();
        samples.resize(len as usize, 0.0);
        if !write_f32s(&mut caller, out, &samples) {
            return 1;
        }
        empty as i32
    }))?;
    e(l.func_wrap(
        "znd",
        "apply_effect",
        |mut caller: Caller<'_, State>, kp: u32, kl: u32, jp: u32, jl: u32, img: u32, w: u32, h: u32| -> i32 {
            let Some(c) = call(&caller) else { return -1 };
            let (Some(kind), Some(json)) = (read_str(&mut caller, kp, kl), read_str(&mut caller, jp, jl)) else { return -1 };
            // 組み込みのエフェクトだけ（プラグインから別のプラグインは呼ばない）
            if super::is_plugin_kind(&kind) || w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
                return -1;
            }
            let params: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(&json) {
                Ok(serde_json::Value::Object(m)) => m,
                _ if json.trim().is_empty() => Default::default(),
                _ => return -1,
            };
            let (w, h) = (w as usize, h as usize);
            let Some(data) = read_f32s(&mut caller, img, w * h * 4) else { return -1 };
            let spec = super::hooks::FilterEffect { kind: kind.clone(), params };
            let scale = (c.ctx.scale as f64).max(1e-6);
            let Some(fx) = super::hooks::build_effect(&spec, w as f64 / scale, h as f64 / scale) else { return -1 };
            let p = fx.eval(c.ctx.time);
            let out = crate::effects::apply(&kind, &p, FrameBuf { width: w, height: h, data }, c.ctx);
            hand_out(c, out)
        },
    ))?;
    e(l.func_wrap("znd", "set_output", |mut caller: Caller<'_, State>, ptr: u32, w: u32, h: u32, ox: i32, oy: i32| -> i32 {
        let Some(c) = call(&caller) else { return 1 };
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return 1;
        }
        let Some(data) = read_f32s(&mut caller, ptr, w as usize * h as usize * 4) else { return 1 };
        *c.output.borrow_mut() = Some((FrameBuf { width: w as usize, height: h as usize, data }, [ox, oy]));
        0
    }))?;
    e(l.func_wrap("znd", "mask_count", |caller: Caller<'_, State>| -> u32 { call(&caller).map_or(0, |c| c.ctx.masks.len() as u32) }))?;
    e(l.func_wrap("znd", "mask_points", |mut caller: Caller<'_, State>, index: u32, out: u32, cap: u32, closed: u32| -> u32 {
        let Some(c) = call(&caller) else { return 0 };
        let Some(path) = c.ctx.masks.get(index as usize) else { return 0 };
        let line = crate::path::flatten(path, 0.25);
        if closed != 0 {
            write_u32(&mut caller, closed, line.closed as u32);
        }
        let n = line.pts.len().min(cap as usize);
        if n > 0 {
            let Some(mem) = memory(&mut caller) else { return 0 };
            if let Some(dst) = mem_slice_mut(mem.data_mut(&mut caller), out, n * 16) {
                for (d, p) in dst.chunks_exact_mut(16).zip(&line.pts) {
                    d[..8].copy_from_slice(&p[0].to_le_bytes());
                    d[8..].copy_from_slice(&p[1].to_le_bytes());
                }
            }
        }
        line.pts.len() as u32
    }))?;
    e(l.func_wrap("znd", "comp_size", |mut caller: Caller<'_, State>, w: u32, h: u32| {
        let Some(c) = call(&caller) else { return };
        let [cw, ch] = c.ctx.comp_size;
        write_u32(&mut caller, w, cw);
        write_u32(&mut caller, h, ch);
    }))?;
    e(l.func_wrap("znd", "param_at", |mut caller: Caller<'_, State>, index: u32, layer_time: f64, out: u32| -> i32 {
        let Some(c) = call(&caller) else { return 1 };
        let Some(param) = c.names.get(index as usize) else { return 1 };
        if !layer_time.is_finite() {
            return 1;
        }
        let at = c.ctx.time + znd_core::Time::from_seconds(layer_time - c.ctx.layer_time);
        let p = (c.ctx.eval_at)(at);
        let color = |v: [f64; 4]| {
            let l = c.ctx.cx.rgb([v[0] as f32, v[1] as f32, v[2] as f32]);
            [l[0] as f64, l[1] as f64, l[2] as f64, v[3]]
        };
        let (values, _) = pack(std::slice::from_ref(param), &p, c.ctx.scale as f64, &color);
        let Some(mem) = memory(&mut caller) else { return 1 };
        match mem_slice_mut(mem.data_mut(&mut caller), out, 32) {
            Some(dst) => {
                for (d, v) in dst.chunks_exact_mut(8).zip(&values) {
                    d.copy_from_slice(&v.to_le_bytes());
                }
                0
            }
            None => 1,
        }
    }))?;
    e(l.func_wrap("znd", "param_image", |caller: Caller<'_, State>, param: u32| -> i32 {
        let Some(c) = call(&caller) else { return -1 };
        let Some(p) = c.names.get(param as usize).filter(|p| p.kind == PARAM_TEXT) else { return -1 };
        let path = c.params.s(&p.name).trim().trim_matches('"').to_string();
        match read_param_image(&path) {
            Some(buf) => hand_out(c, buf),
            None => -1,
        }
    }))?;
    e(l.func_wrap("znd", "log", |mut caller: Caller<'_, State>, ptr: u32, len: u32| {
        // 開発用。出しすぎないように 1 インスタンス 100 行まで
        if caller.data().logs >= 100 {
            return;
        }
        caller.data_mut().logs += 1;
        if let Some(s) = read_str(&mut caller, ptr, len.min(1000)) {
            eprintln!("[プラグイン] {s}");
        }
    }))?;
    Ok(l)
}

/// 文字列パラメータ（利用者がファイルの選択で入れたパス）の画像を、ホストが読んで画素だけを渡す。
/// プラグイン自身はファイルに触れない。読めるのは画像の形式・64MB・一辺 16384 px までのファイルだけで、
/// 画素はファイルの値そのまま（0〜1、色空間の変換なし、ストレートアルファ）。
fn read_param_image(path: &str) -> Option<FrameBuf> {
    if path.is_empty() {
        return None;
    }
    let path = crate::effects::color::resolve_path(path);
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tga" | "tif" | "tiff" | "exr") {
        return None;
    }
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 64 << 20 {
        return None;
    }
    let img = image::ImageReader::open(&path).ok()?.with_guessed_format().ok()?;
    let (w, h) = img.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    let rgba = image::ImageReader::open(&path).ok()?.with_guessed_format().ok()?.decode().ok()?.to_rgba32f();
    Some(FrameBuf { width: w as usize, height: h as usize, data: rgba.into_raw() })
}

fn instantiate(module: &Module) -> Result<Inst, String> {
    static LINKER: OnceLock<Result<Linker<State>, String>> = OnceLock::new();
    let linker = LINKER.get_or_init(linker).as_ref().map_err(|e| e.clone())?;
    // 読み込める関数は znd のホスト関数だけ。WASI（ファイル・ネットワークなど）は渡さない。
    // wasm-bindgen（ブラウザー向けの橋渡し）の関数は、呼ばれたらトラップする空の関数でつなぐ（何の権限も与えない）
    let mut stubbed = false;
    for imp in module.imports() {
        match imp.module() {
            "znd" => {}
            m if m.starts_with("__wbindgen") => stubbed = true,
            m => {
                return Err(format!(
                    "使えない関数を読み込もうとしています（{m}::{}）。znd のプラグインはサンドボックスの中で動き、ファイル・ネットワーク・別のプログラムには触れられません",
                    imp.name()
                ))
            }
        }
    }
    let local;
    let linker = if stubbed {
        let mut l = linker.clone();
        l.define_unknown_imports_as_traps(module).map_err(|e| e.to_string())?;
        local = l;
        &local
    } else {
        linker
    };
    let mut store = new_store();
    let instance: Instance = linker.instantiate(&mut store, module).map_err(|e| format!("インスタンスを作れません: {e}"))?;
    let memory = instance.get_memory(&mut store, "memory").ok_or("memory がありません")?;
    let alloc = instance.get_typed_func::<u32, u32>(&mut store, "znd_alloc").map_err(|e| format!("znd_alloc: {e}"))?;
    let free = instance.get_typed_func::<(u32, u32), ()>(&mut store, "znd_free").map_err(|e| format!("znd_free: {e}"))?;
    let render = instance.get_typed_func::<(u32, u32), i32>(&mut store, "znd_render").map_err(|e| format!("znd_render: {e}"))?;
    let audio = instance.get_typed_func(&mut store, "znd_process_audio").ok();
    Ok(Inst { store, instance, memory, alloc, free, render, audio })
}

impl Inst {
    fn reset_deadline(&mut self) {
        self.store.set_epoch_deadline(deadline_ticks());
    }

    /// ゲストのメモリにバイト列を置く（確保して書く）。
    fn put(&mut self, bytes: &[u8]) -> Result<u32, String> {
        let len = u32::try_from(bytes.len()).map_err(|_| "大きすぎます")?;
        let ptr = self.alloc.call(&mut self.store, len.max(1)).map_err(|e| e.to_string())?;
        if ptr == 0 {
            return Err("メモリを確保できません".into());
        }
        let dst = mem_slice_mut(self.memory.data_mut(&mut self.store), ptr, bytes.len()).ok_or("確保した範囲が不正です")?;
        dst.copy_from_slice(bytes);
        Ok(ptr)
    }
}

// ---------- 定義（znd_describe の JSON） ----------

#[derive(Deserialize)]
struct DescJson {
    abi: u32,
    #[serde(default)]
    vendor: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    license: String,
    effects: Vec<EffectJson>,
}

#[derive(Deserialize)]
struct EffectJson {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    flags: u32,
    #[serde(default)]
    params: Vec<ParamJson>,
}

#[derive(Deserialize)]
struct ParamJson {
    id: String,
    #[serde(default)]
    label: String,
    kind: u32,
    #[serde(default)]
    flags: u32,
    #[serde(default)]
    def: [f64; 4],
    #[serde(default)]
    min: f64,
    #[serde(default)]
    max: f64,
    #[serde(default)]
    choices: String,
}

/// モジュール 1 つ（複数のエフェクトを含む）。
struct WasmLib {
    module: Module,
    /// スレッドセーフでないエフェクトと音声の処理が使う、1 つだけのインスタンス（状態を持てる）。
    primary: Mutex<Option<Inst>>,
    /// スレッドセーフなエフェクトが並列に使うインスタンス。
    pool: Mutex<Vec<Inst>>,
    pool_created: AtomicUsize,
}

impl WasmLib {
    /// インスタンスを借りて `f` を呼ぶ。トラップ（エラー）したインスタンスは捨てる。
    fn with<R>(&self, shared_ok: bool, f: impl FnOnce(&mut Inst) -> Result<R, String>) -> Result<R, String> {
        if shared_ok {
            let taken = self.pool.lock().unwrap_or_else(|e| e.into_inner()).pop();
            let inst = match taken {
                Some(i) => Some(i),
                None if self.pool_created.fetch_add(1, Ordering::SeqCst) < POOL_MAX => Some(instantiate(&self.module).inspect_err(|_| {
                    self.pool_created.fetch_sub(1, Ordering::SeqCst);
                })?),
                None => {
                    self.pool_created.fetch_sub(1, Ordering::SeqCst);
                    None
                }
            };
            if let Some(mut inst) = inst {
                inst.reset_deadline();
                let r = f(&mut inst);
                if r.is_ok() {
                    self.pool.lock().unwrap_or_else(|e| e.into_inner()).push(inst);
                } else {
                    self.pool_created.fetch_sub(1, Ordering::SeqCst);
                }
                return r;
            }
        }
        let mut guard = self.primary.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(instantiate(&self.module)?);
        }
        let inst = guard.as_mut().expect("作ったばかり");
        inst.reset_deadline();
        let r = f(inst);
        if r.is_err() {
            *guard = None;
        }
        r
    }
}

struct WasmEffect {
    info: PluginInfo,
    lib: Arc<WasmLib>,
    index: u32,
    params: Arc<Vec<Param>>,
    thread_safe: bool,
    audio_only: bool,
}

struct WasmAudio {
    lib: Arc<WasmLib>,
    index: u32,
    params: Arc<Vec<Param>>,
}

/// パラメータの値をゲストのメモリの並び（f64 のリトルエンディアン）にする。
fn values_bytes(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// 文字列を置き、(ポインタ, 長さ) の表を置いて、その表のポインタを返す。
fn put_strings(inst: &mut Inst, strings: &[Option<String>], owned: &mut Vec<(u32, u32)>) -> Result<u32, String> {
    let mut table = Vec::with_capacity(strings.len() * 8);
    for s in strings {
        let (ptr, len) = match s {
            Some(s) if !s.is_empty() => {
                let p = inst.put(s.as_bytes())?;
                owned.push((p, s.len() as u32));
                (p, s.len() as u32)
            }
            _ => (0, 0),
        };
        table.extend(ptr.to_le_bytes());
        table.extend(len.to_le_bytes());
    }
    let t = inst.put(&table)?;
    owned.push((t, table.len().max(1) as u32));
    Ok(t)
}

fn free_all(inst: &mut Inst, owned: &[(u32, u32)]) {
    for (p, l) in owned {
        let _ = inst.free.call(&mut inst.store, (*p, *l));
    }
}

fn strings_of(params: &[Param], p: &EvalParams) -> Vec<Option<String>> {
    params.iter().map(|q| (q.kind == PARAM_TEXT).then(|| p.s(&q.name).to_string())).collect()
}

impl Effect for WasmEffect {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn apply(&self, p: &EvalParams, buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
        if self.audio_only || buf.width == 0 || buf.height == 0 {
            return buf;
        }
        let s = ctx.scale as f64;
        let color = |c: [f64; 4]| {
            let l = ctx.cx.rgb([c[0] as f32, c[1] as f32, c[2] as f32]);
            [l[0] as f64, l[1] as f64, l[2] as f64, c[3]]
        };
        let (values, _) = pack(&self.params, p, s, &color);
        let strings = strings_of(&self.params, p);
        let call_state = Call {
            ctx,
            params: p,
            names: &self.params,
            images: RefCell::default(),
            output: RefCell::default(),
            size: (buf.width, buf.height),
        };
        let result = self.lib.with(self.thread_safe, |inst| {
            let mut owned = Vec::new();
            let r = (|| -> Result<Option<Vec<f32>>, String> {
                let pixels: Vec<u8> = buf.data.iter().flat_map(|v| v.to_le_bytes()).collect();
                let rgba = inst.put(&pixels)?;
                owned.push((rgba, pixels.len() as u32));
                let vals = inst.put(&values_bytes(&values))?;
                owned.push((vals, (values.len() * 8).max(1) as u32));
                let strs = put_strings(inst, &strings, &mut owned)?;
                let mut args = Vec::with_capacity(72);
                for v in [rgba, buf.width as u32, buf.height as u32, self.params.len() as u32] {
                    args.extend(v.to_le_bytes());
                }
                for v in [ctx.time.seconds(), ctx.frame_rate.fps(), s, ctx.layer_time, ctx.layer_duration] {
                    args.extend(v.to_le_bytes());
                }
                for v in [ctx.seed, vals, strs, 0] {
                    args.extend(v.to_le_bytes());
                }
                let a = inst.put(&args)?;
                owned.push((a, args.len() as u32));
                // SAFETY: `call_state` はこのクロージャの外のスタックにあり、render が戻るまで生きている。戻ったら null に戻す。
                inst.store.data_mut().call = std::ptr::from_ref(&call_state).cast::<Call<'static, 'static>>();
                let status = inst.render.call(&mut inst.store, (self.index, a));
                inst.store.data_mut().call = std::ptr::null();
                let status = status.map_err(|e| e.to_string())?;
                if status != 0 {
                    return Ok(None);
                }
                // 結果（大きさを変えていなければ、渡したバッファがそのまま結果）
                let src = mem_slice(inst.memory.data(&inst.store), rgba, pixels.len()).ok_or("結果を読めません")?;
                Ok(Some(src.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()))
            })();
            free_all(inst, &owned);
            r
        });
        let output = call_state.output.borrow_mut().take();
        match result {
            Ok(Some(data)) => match output {
                Some((out, [ox, oy])) if out.data.iter().all(|v| v.is_finite()) => {
                    let o = ctx.offset.get();
                    ctx.offset.set([o[0] + ox as f64, o[1] + oy as f64]);
                    out
                }
                Some(_) => buf,
                None if data.len() == buf.data.len() && data.iter().all(|v| v.is_finite()) => FrameBuf { width: buf.width, height: buf.height, data },
                None => buf,
            },
            Ok(None) => buf,
            Err(e) => {
                super::guard::report_error(&self.info.kind, &e);
                buf
            }
        }
    }
}

impl AudioProcessor for WasmAudio {
    fn process(&self, p: &EvalParams, block: AudioBlock) {
        let (values, _) = pack(&self.params, p, 1.0, &|c| c);
        let strings = strings_of(&self.params, p);
        let n = block.samples.len();
        let result = self.lib.with(false, |inst| {
            let Some(audio) = inst.audio.clone() else { return Ok(None) };
            let mut owned = Vec::new();
            let r = (|| -> Result<Option<Vec<f32>>, String> {
                let bytes: Vec<u8> = block.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
                let smp = inst.put(&bytes)?;
                owned.push((smp, bytes.len().max(1) as u32));
                let vals = inst.put(&values_bytes(&values))?;
                owned.push((vals, (values.len() * 8).max(1) as u32));
                let strs = put_strings(inst, &strings, &mut owned)?;
                let mut args = Vec::with_capacity(64);
                for v in [smp, (n / 2) as u32, block.sample_rate, self.params.len() as u32] {
                    args.extend(v.to_le_bytes());
                }
                args.extend(block.start.to_le_bytes());
                args.extend(block.time.seconds().to_le_bytes());
                args.extend(block.instance.to_le_bytes());
                args.extend(block.length.to_le_bytes());
                for v in [block.seed, vals, strs, 0] {
                    args.extend(v.to_le_bytes());
                }
                let a = inst.put(&args)?;
                owned.push((a, args.len() as u32));
                let status = audio.call(&mut inst.store, (self.index, a)).map_err(|e| e.to_string())?;
                if status != 0 {
                    return Ok(None);
                }
                let src = mem_slice(inst.memory.data(&inst.store), smp, bytes.len()).ok_or("結果を読めません")?;
                Ok(Some(src.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()))
            })();
            free_all(inst, &owned);
            r
        });
        if let Ok(Some(out)) = result {
            if out.len() == n && out.iter().all(|v| v.is_finite()) {
                block.samples.copy_from_slice(&out);
            }
        }
    }
}

/// モジュールの定義を読み、エフェクトを登録する。
pub(crate) fn load(path: &Path) -> Result<Vec<Arc<dyn Effect>>, String> {
    let module = compile(path)?;
    let mut inst = instantiate(&module)?;
    let abi = inst_call0::<i32>(&mut inst, "znd_abi")?;
    if abi as u32 != WASM_ABI {
        return Err(format!("WASM ABI {abi} は未対応です（{WASM_ABI} に対応）"));
    }
    let packed = inst_call0::<i64>(&mut inst, "znd_describe")?;
    let (ptr, len) = ((packed as u64 >> 32) as u32, (packed as u64 & 0xffff_ffff) as usize);
    if len > 4 << 20 {
        return Err("定義が大きすぎます".into());
    }
    let json = mem_slice(inst.memory.data(&inst.store), ptr, len).ok_or("定義を読めません")?.to_vec();
    let desc: DescJson = serde_json::from_slice(&json).map_err(|e| format!("定義の書式が不正です: {e}"))?;
    if desc.abi != WASM_ABI {
        return Err(format!("定義の ABI {} は未対応です", desc.abi));
    }
    if desc.effects.len() > 1024 {
        return Err("エフェクトの定義が多すぎます".into());
    }
    let has_audio = inst.audio.is_some();
    let lib = Arc::new(WasmLib { module, primary: Mutex::new(Some(inst)), pool: Mutex::default(), pool_created: AtomicUsize::new(0) });
    let mut out: Vec<Arc<dyn Effect>> = Vec::new();
    for (index, e) in desc.effects.iter().enumerate() {
        if !valid_id(&e.id) {
            return Err(format!("エフェクトの ID「{}」が不正です（英数字と . _ - のみ）", e.id));
        }
        if e.params.len() > 256 {
            return Err(format!("{}: パラメータが多すぎます", e.id));
        }
        let is_audio = e.flags & EFFECT_AUDIO != 0;
        if is_audio && !has_audio {
            return Err(format!("{}: 音声エフェクトには znd_process_audio が必要です", e.id));
        }
        let mut params = Vec::new();
        let mut defs = Vec::new();
        for p in &e.params {
            let name = p.id.clone();
            if !valid_id(&name) || params.iter().any(|q: &Param| q.name == name) {
                return Err(format!("{}: パラメータの ID「{name}」が不正か、重複しています", e.id));
            }
            let label = Some(p.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| name.clone());
            let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
            let def = p.def.map(fin);
            let pd = match p.kind {
                PARAM_FLOAT => {
                    let (lo, hi) = if p.max > p.min && p.min.is_finite() && p.max.is_finite() { (p.min, p.max) } else { (-1e6, 1e6) };
                    DynPd::F(def[0], lo, hi)
                }
                PARAM_BOOL => DynPd::B(def[0] != 0.0),
                PARAM_CHOICE => {
                    let labels: Vec<String> = p.choices.split('|').filter(|s| !s.is_empty()).map(String::from).collect();
                    if labels.is_empty() {
                        return Err(format!("{}.{name}: 選択肢がありません", e.id));
                    }
                    DynPd::I((def[0].max(0.0) as u32).min(labels.len() as u32 - 1), labels)
                }
                PARAM_COLOR => DynPd::C(def),
                PARAM_POINT => DynPd::P(def[0], def[1]),
                PARAM_TEXT => DynPd::S,
                PARAM_LAYER => DynPd::L,
                other => return Err(format!("{}.{name}: 未対応のパラメータ種別です（{other}）", e.id)),
            };
            params.push(Param { name: name.clone(), kind: p.kind, pixels: p.flags & PARAM_PIXELS != 0 });
            defs.push((name, label, pd));
        }
        let kind = format!("plugin:{}", e.id);
        let label = Some(e.name.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| e.id.clone());
        let default_category = if is_audio { "音声（プラグイン）" } else { "プラグイン" };
        let category = Some(e.category.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| default_category.into());
        register_effect(DynEffect {
            kind: kind.clone(),
            label: label.clone(),
            category: category.clone(),
            time_dependent: e.flags & EFFECT_TIME_DEPENDENT != 0,
            params: defs,
        });
        if e.flags & EFFECT_READS_BELOW != 0 {
            znd_core::effect::mark_reads_below(&kind);
        }
        let params = Arc::new(params);
        if is_audio {
            register_audio_processor(&kind, Arc::new(WasmAudio { lib: lib.clone(), index: index as u32, params: params.clone() }));
        }
        out.push(Arc::new(WasmEffect {
            info: PluginInfo {
                kind,
                label,
                category,
                vendor: desc.vendor.clone(),
                version: desc.version.clone(),
                license: desc.license.clone(),
                path: path.to_path_buf(),
                api: PluginApi::Wasm,
                param_count: params.len(),
            },
            lib: lib.clone(),
            index: index as u32,
            params,
            thread_safe: e.flags & EFFECT_THREAD_SAFE != 0,
            audio_only: is_audio,
        }));
    }
    Ok(out)
}

/// 引数なしの関数を呼ぶ。
fn inst_call0<R: wasmtime::WasmResults>(inst: &mut Inst, name: &str) -> Result<R, String> {
    let f: TypedFunc<(), R> = inst.instance.get_typed_func(&mut inst.store, name).map_err(|e| format!("{name}: {e}"))?;
    inst.reset_deadline();
    f.call(&mut inst.store, ()).map_err(|e| format!("{name}: {e}"))
}
