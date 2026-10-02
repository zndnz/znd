//! サンプルプラグイン（Rust 版）: 流れる走査線。
//!
//!   rustc --crate-type cdylib -O -o scanline.dylib scanline.rs
//!
//! C のヘッダー（sdk/znd_plugin.h）と同じ並びの構造体を `#[repr(C)]` で定義すれば、どの言語でも書ける。

use std::ffi::c_char;

#[repr(C)]
pub struct ParamDesc {
    id: *const c_char,
    label: *const c_char,
    kind: u32,
    flags: u32,
    def: [f64; 4],
    min: f64,
    max: f64,
    choices: *const c_char,
}

#[repr(C)]
pub struct EffectDesc {
    id: *const c_char,
    name: *const c_char,
    category: *const c_char,
    flags: u32,
    param_count: u32,
    params: *const ParamDesc,
}

#[repr(C)]
pub struct PluginDesc {
    abi_version: u32,
    vendor: *const c_char,
    version: *const c_char,
    license: *const c_char,
    effect_count: u32,
    effects: *const EffectDesc,
}

#[repr(C)]
pub struct RenderArgs {
    rgba: *mut f32,
    width: u32,
    height: u32,
    time: f64,
    fps: f64,
    scale: f64,
    seed: u32,
    param_count: u32,
    values: *const f64,
}

// 定義は読み取り専用の静的データなので、スレッド間で共有してよい
struct Shared<T>(T);
unsafe impl<T> Sync for Shared<T> {}

macro_rules! c {
    ($s:literal) => {
        concat!($s, "\0").as_ptr() as *const c_char
    };
}

const PIXELS: u32 = 1;
const TIME_DEPENDENT: u32 = 1;
const THREAD_SAFE: u32 = 2;

static PARAMS: Shared<[ParamDesc; 3]> = Shared([
    ParamDesc { id: c!("spacing"), label: c!("間隔（px）"), kind: 0, flags: PIXELS, def: [8.0, 0.0, 0.0, 0.0], min: 2.0, max: 500.0, choices: std::ptr::null() },
    ParamDesc { id: c!("darkness"), label: c!("濃さ"), kind: 0, flags: 0, def: [0.5, 0.0, 0.0, 0.0], min: 0.0, max: 1.0, choices: std::ptr::null() },
    ParamDesc { id: c!("speed"), label: c!("流れる速さ（px/秒）"), kind: 0, flags: PIXELS, def: [30.0, 0.0, 0.0, 0.0], min: -2000.0, max: 2000.0, choices: std::ptr::null() },
]);

static EFFECTS: Shared<[EffectDesc; 1]> = Shared([EffectDesc {
    id: c!("org.znd.example.scanline"),
    name: c!("流れる走査線"),
    category: c!("サンプル"),
    flags: TIME_DEPENDENT | THREAD_SAFE,
    param_count: 3,
    params: PARAMS.0.as_ptr(),
}]);

static DESC: Shared<PluginDesc> = Shared(PluginDesc {
    abi_version: 2,
    vendor: c!("znd サンプル"),
    version: c!("1.0"),
    license: c!("MIT OR Apache-2.0"),
    effect_count: 1,
    effects: EFFECTS.0.as_ptr(),
});

#[no_mangle]
pub extern "C" fn znd_plugin_describe() -> *const PluginDesc {
    &DESC.0
}

/// # Safety
/// `args` はホストが用意した有効な構造体であること。
#[no_mangle]
pub unsafe extern "C" fn znd_plugin_render(effect: u32, args: *const RenderArgs) -> i32 {
    if effect != 0 || args.is_null() {
        return 1;
    }
    let a = &*args;
    let v = std::slice::from_raw_parts(a.values, a.param_count as usize * 4);
    let px = std::slice::from_raw_parts_mut(a.rgba, a.width as usize * a.height as usize * 4);
    let (spacing, darkness, speed) = (v[0].max(1.0), v[4], v[8]);
    for (y, row) in px.chunks_exact_mut(a.width as usize * 4).enumerate() {
        let phase = ((y as f64 + 0.5 + speed * a.time) / spacing).rem_euclid(1.0);
        if phase < 0.5 {
            let gain = (1.0 - darkness) as f32;
            for p in row.chunks_exact_mut(4) {
                p[0] *= gain;
                p[1] *= gain;
                p[2] *= gain;
            }
        }
    }
    0
}
