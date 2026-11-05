//! znd ネイティブプラグイン（`sdk/znd_plugin.h`、ABI 2 / 3）の読み込みと実行。
//!
//! ABI 3 で増えたもの（ABI 2 のプラグインもそのまま読み込める）:
//! - `ZndRenderArgs` の後半: 文字列パラメータ、レイヤーの経過時間、ホスト関数表（`ZndHost`）
//! - ホスト関数: 別時刻の自分の絵・他レイヤーの絵・音声の取得、組み込みエフェクトの呼び出し、出力サイズの変更
//! - パラメータ: TEXT（文字列）/ LAYER（レイヤー参照）
//! - 音声エフェクト（`ZND_EFFECT_AUDIO` と `znd_plugin_process_audio`）

use super::{Effect, PluginApi, PluginInfo};
use crate::effects::FxContext;
use crate::frame::FrameBuf;
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::sync::{Arc, Mutex};
use znd_core::effect::{register_audio_processor, register_effect, AudioBlock, AudioProcessor, DynEffect, DynPd, EvalParams};

pub const ABI_VERSION: u32 = 3;
const ABI_MIN: u32 = 2;

#[repr(C)]
pub struct ParamDesc {
    pub id: *const c_char,
    pub label: *const c_char,
    pub kind: u32,
    pub flags: u32,
    pub def: [f64; 4],
    pub min: f64,
    pub max: f64,
    pub choices: *const c_char,
}

#[repr(C)]
pub struct EffectDesc {
    pub id: *const c_char,
    pub name: *const c_char,
    pub category: *const c_char,
    pub flags: u32,
    pub param_count: u32,
    pub params: *const ParamDesc,
}

#[repr(C)]
pub struct PluginDesc {
    pub abi_version: u32,
    pub vendor: *const c_char,
    pub version: *const c_char,
    pub license: *const c_char,
    pub effect_count: u32,
    pub effects: *const EffectDesc,
}

/// ホストが確保した画像。`rgba` は `width*height*4` 要素で、`render` から戻るまで有効。
#[repr(C)]
pub struct Image {
    pub rgba: *mut f32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
pub struct Host {
    pub ctx: *mut c_void,
    pub frame_at: unsafe extern "C" fn(*mut c_void, f64, *mut Image) -> i32,
    pub layer_image: unsafe extern "C" fn(*mut c_void, u32, *mut Image) -> i32,
    pub audio: unsafe extern "C" fn(*mut c_void, u32, *mut f32, u32) -> i32,
    pub apply_effect: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *mut Image) -> i32,
    pub set_output: unsafe extern "C" fn(*mut c_void, u32, u32, i32, i32, *mut Image) -> i32,
    pub mask_count: unsafe extern "C" fn(*mut c_void) -> u32,
    pub mask_points: unsafe extern "C" fn(*mut c_void, u32, *mut f64, u32, *mut u32) -> u32,
    pub comp_size: unsafe extern "C" fn(*mut c_void, *mut u32, *mut u32),
    pub param_at: unsafe extern "C" fn(*mut c_void, u32, f64, *mut f64) -> i32,
    pub backdrop: unsafe extern "C" fn(*mut c_void, *mut Image) -> i32,
    pub layer_source: unsafe extern "C" fn(*mut c_void, u32, *mut Image) -> i32,
}

#[repr(C)]
pub struct RenderArgs {
    pub rgba: *mut f32,
    pub width: u32,
    pub height: u32,
    pub time: f64,
    pub fps: f64,
    pub scale: f64,
    pub seed: u32,
    pub param_count: u32,
    pub values: *const f64,
    // ---- ABI 3 ----
    pub strings: *const *const c_char,
    pub layer_time: f64,
    pub layer_duration: f64,
    pub host: *const Host,
}

#[repr(C)]
pub struct AudioArgs {
    pub samples: *mut f32,
    pub frames: u32,
    pub sample_rate: u32,
    pub start: i64,
    pub time: f64,
    pub instance: u64,
    pub seed: u32,
    pub param_count: u32,
    pub values: *const f64,
    pub strings: *const *const c_char,
    pub length: i64,
}

const PARAM_FLOAT: u32 = 0;
const PARAM_BOOL: u32 = 1;
const PARAM_CHOICE: u32 = 2;
const PARAM_COLOR: u32 = 3;
const PARAM_POINT: u32 = 4;
const PARAM_TEXT: u32 = 5;
const PARAM_LAYER: u32 = 6;
const PARAM_PIXELS: u32 = 1;
const EFFECT_TIME_DEPENDENT: u32 = 1;
const EFFECT_THREAD_SAFE: u32 = 2;
const EFFECT_AUDIO: u32 = 4;
const EFFECT_READS_BELOW: u32 = 8;
/// 出力画像の一辺の上限（誤った値で巨大な確保をしないため）。
const MAX_SIDE: u32 = 16384;

type DescribeFn = unsafe extern "C" fn() -> *const PluginDesc;
type RenderFn = unsafe extern "C" fn(u32, *const RenderArgs) -> i32;
type AudioFn = unsafe extern "C" fn(u32, *const AudioArgs) -> i32;

struct Lib {
    // ライブラリを保持し続ける（関数ポインタの寿命を保証する）
    _lib: libloading::Library,
    render: RenderFn,
    audio: Option<AudioFn>,
    abi: u32,
    /// スレッドセーフを宣言していないエフェクトは、これで直列化する。
    lock: Mutex<()>,
}

// 関数ポインタとライブラリハンドルはスレッド間で共有してよい（再入できないものは lock で守る）。
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

struct Param {
    name: String,
    kind: u32,
    pixels: bool,
}

struct NativeEffect {
    info: PluginInfo,
    lib: Arc<Lib>,
    index: u32,
    params: Arc<Vec<Param>>,
    thread_safe: bool,
    audio_only: bool,
}

struct NativeAudio {
    lib: Arc<Lib>,
    index: u32,
    params: Arc<Vec<Param>>,
    thread_safe: bool,
}

/// NUL 終端文字列を読む。NULL は空文字列。
unsafe fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub(crate) fn load(path: &Path) -> Result<Vec<Arc<dyn Effect>>, String> {
    // SAFETY: 動的ライブラリの読み込みは初期化コードを実行する。プラグインフォルダに置かれたファイルに限る。
    unsafe {
        let lib = libloading::Library::new(path).map_err(|e| format!("読み込めません: {e}"))?;
        let describe: libloading::Symbol<DescribeFn> = lib
            .get(b"znd_plugin_describe")
            .map_err(|_| "znd_plugin_describe がありません（znd のプラグインではないか、ABI 1 の古い形式です）".to_string())?;
        let render: RenderFn = *lib.get::<RenderFn>(b"znd_plugin_render").map_err(|_| "znd_plugin_render がありません".to_string())?;
        let audio: Option<AudioFn> = lib.get::<AudioFn>(b"znd_plugin_process_audio").ok().map(|s| *s);
        let desc = describe();
        if desc.is_null() {
            return Err("プラグインの定義を取得できません".into());
        }
        let desc = &*desc;
        if !(ABI_MIN..=ABI_VERSION).contains(&desc.abi_version) {
            return Err(format!("ABI バージョン {} は未対応です（{ABI_MIN}〜{ABI_VERSION} に対応）", desc.abi_version));
        }
        if desc.effect_count > 1024 || (desc.effect_count > 0 && desc.effects.is_null()) {
            return Err("エフェクトの定義が不正です".into());
        }
        let abi = desc.abi_version;
        let (vendor, version, license) = (text(desc.vendor), text(desc.version), text(desc.license));
        let descs = std::slice::from_raw_parts(desc.effects, desc.effect_count as usize);
        let lib = Arc::new(Lib { _lib: lib, render, audio, abi, lock: Mutex::new(()) });
        let mut out: Vec<Arc<dyn Effect>> = Vec::new();
        for (index, e) in descs.iter().enumerate() {
            let id = text(e.id);
            if !valid_id(&id) {
                return Err(format!("エフェクトの ID「{id}」が不正です（英数字と . _ - のみ）"));
            }
            if e.param_count > 256 || (e.param_count > 0 && e.params.is_null()) {
                return Err(format!("{id}: パラメータの定義が不正です"));
            }
            let is_audio = e.flags & EFFECT_AUDIO != 0;
            if is_audio && (abi < 3 || audio.is_none()) {
                return Err(format!("{id}: 音声エフェクトには ABI 3 と znd_plugin_process_audio が必要です"));
            }
            // パラメータの無いエフェクトは params が NULL でもよい
            let pdescs = if e.param_count == 0 { &[][..] } else { std::slice::from_raw_parts(e.params, e.param_count as usize) };
            let mut params = Vec::new();
            let mut defs = Vec::new();
            for p in pdescs {
                let name = text(p.id);
                if !valid_id(&name) || params.iter().any(|q: &Param| q.name == name) {
                    return Err(format!("{id}: パラメータの ID「{name}」が不正か、重複しています"));
                }
                let label = Some(text(p.label)).filter(|l| !l.is_empty()).unwrap_or_else(|| name.clone());
                let pd = match p.kind {
                    PARAM_FLOAT => {
                        let (lo, hi) = if p.max > p.min { (p.min, p.max) } else { (-1e6, 1e6) };
                        DynPd::F(p.def[0], lo, hi)
                    }
                    PARAM_BOOL => DynPd::B(p.def[0] != 0.0),
                    PARAM_CHOICE => {
                        let labels: Vec<String> = text(p.choices).split('|').filter(|s| !s.is_empty()).map(String::from).collect();
                        if labels.is_empty() {
                            return Err(format!("{id}.{name}: 選択肢がありません"));
                        }
                        DynPd::I((p.def[0].max(0.0) as u32).min(labels.len() as u32 - 1), labels)
                    }
                    PARAM_COLOR => DynPd::C(p.def),
                    PARAM_POINT => DynPd::P(p.def[0], p.def[1]),
                    PARAM_TEXT if abi >= 3 => DynPd::S,
                    PARAM_LAYER if abi >= 3 => DynPd::L,
                    other => return Err(format!("{id}.{name}: 未対応のパラメータ種別です（{other}）")),
                };
                params.push(Param { name: name.clone(), kind: p.kind, pixels: p.flags & PARAM_PIXELS != 0 });
                defs.push((name, label, pd));
            }
            let kind = format!("plugin:{id}");
            let label = Some(text(e.name)).filter(|l| !l.is_empty()).unwrap_or_else(|| id.clone());
            let default_category = if is_audio { "音声（プラグイン）" } else { "プラグイン" };
            let category = Some(text(e.category)).filter(|c| !c.is_empty()).unwrap_or_else(|| default_category.into());
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
            let thread_safe = e.flags & EFFECT_THREAD_SAFE != 0;
            if is_audio {
                register_audio_processor(
                    &kind,
                    Arc::new(NativeAudio { lib: lib.clone(), index: index as u32, params: params.clone(), thread_safe }),
                );
            }
            out.push(Arc::new(NativeEffect {
                info: PluginInfo {
                    kind,
                    label,
                    category,
                    vendor: vendor.clone(),
                    version: version.clone(),
                    license: license.clone(),
                    path: path.to_path_buf(),
                    api: PluginApi::Native,
                    param_count: params.len(),
                },
                lib: lib.clone(),
                index: index as u32,
                params,
                thread_safe,
                audio_only: is_audio,
            }));
        }
        Ok(out)
    }
}

/// パラメータを ABI の並び（1 つあたり double 4 個 + 文字列 1 個）に詰める。
fn pack(params: &[Param], p: &EvalParams, scale: f64, color: &dyn Fn([f64; 4]) -> [f64; 4]) -> (Vec<f64>, Vec<Option<CString>>) {
    let mut values = vec![0.0f64; params.len() * 4];
    let mut strings = Vec::with_capacity(params.len());
    for (param, v) in params.iter().zip(values.chunks_exact_mut(4)) {
        let mut s = None;
        match param.kind {
            PARAM_FLOAT => v[0] = p.f(&param.name) * if param.pixels { scale } else { 1.0 },
            PARAM_BOOL => v[0] = p.b(&param.name) as u8 as f64,
            PARAM_CHOICE => v[0] = p.i(&param.name) as f64,
            PARAM_COLOR => v.copy_from_slice(&color(p.c(&param.name))),
            PARAM_POINT => {
                let pt = p.p(&param.name);
                (v[0], v[1]) = (pt[0] * scale, pt[1] * scale);
            }
            PARAM_TEXT => s = Some(CString::new(p.s(&param.name).replace('\0', "")).unwrap_or_default()),
            PARAM_LAYER => v[0] = p.layer(&param.name).is_some() as u8 as f64,
            _ => {}
        }
        strings.push(s);
    }
    (values, strings)
}

/// `render` の間だけ生きる、ホスト関数の状態。
struct HostState<'a, 'b> {
    ctx: &'a FxContext<'b>,
    params: &'a EvalParams,
    names: &'a [Param],
    /// プラグインに渡した画像（`render` から戻るまで保持する）。
    keep: RefCell<Vec<FrameBuf>>,
    /// `set_output` で作った出力画像と、その左上の位置（入力の画素座標）。
    output: RefCell<Option<(FrameBuf, [i32; 2])>>,
    /// 入力のバッファの大きさ。
    size: (usize, usize),
}

impl HostState<'_, '_> {
    fn hand_out(&self, mut buf: FrameBuf, out: *mut Image) -> i32 {
        if out.is_null() {
            return 1;
        }
        let img = Image { rgba: buf.data.as_mut_ptr(), width: buf.width as u32, height: buf.height as u32 };
        // Vec の中身はヒープにあるので、Vec ごと移動しても同じ場所にある
        self.keep.borrow_mut().push(buf);
        // SAFETY: out はプラグインが渡した書き込み先。
        unsafe { out.write(img) };
        0
    }
}

unsafe fn state<'a>(ctx: *mut c_void) -> &'a HostState<'a, 'a> {
    &*(ctx as *const HostState)
}

unsafe extern "C" fn host_frame_at(ctx: *mut c_void, time: f64, out: *mut Image) -> i32 {
    let s = state(ctx);
    match (s.ctx.fetch_self)(znd_core::Time::from_seconds(time)) {
        Some(buf) => s.hand_out(buf, out),
        None => 1,
    }
}

unsafe extern "C" fn host_backdrop(ctx: *mut c_void, out: *mut Image) -> i32 {
    let s = state(ctx);
    match (s.ctx.fetch_below)(s.size.0, s.size.1) {
        Some(buf) => s.hand_out(buf, out),
        None => 1,
    }
}

unsafe extern "C" fn host_layer_image(ctx: *mut c_void, param: u32, out: *mut Image) -> i32 {
    let s = state(ctx);
    let Some(p) = s.names.get(param as usize).filter(|p| p.kind == PARAM_LAYER) else { return 1 };
    let Some(id) = s.params.layer(&p.name) else { return 1 };
    match (s.ctx.fetch_layer)(id) {
        Some(buf) => s.hand_out(buf, out),
        None => 1,
    }
}

unsafe extern "C" fn host_layer_source(ctx: *mut c_void, param: u32, out: *mut Image) -> i32 {
    let s = state(ctx);
    let Some(p) = s.names.get(param as usize).filter(|p| p.kind == PARAM_LAYER) else { return 1 };
    let Some(id) = s.params.layer(&p.name) else { return 1 };
    match (s.ctx.fetch_source)(id) {
        Some(buf) => s.hand_out(buf, out),
        None => 1,
    }
}

unsafe extern "C" fn host_audio(ctx: *mut c_void, param: u32, out: *mut f32, len: u32) -> i32 {
    let s = state(ctx);
    if out.is_null() || len == 0 || len > 1 << 22 {
        return 1;
    }
    let Some(p) = s.names.get(param as usize).filter(|p| p.kind == PARAM_LAYER) else { return 1 };
    let Some(id) = s.params.layer(&p.name) else { return 1 };
    let samples = (s.ctx.audio)(id, len as usize);
    let dst = std::slice::from_raw_parts_mut(out, len as usize);
    dst.fill(0.0);
    dst.iter_mut().zip(&samples).for_each(|(d, v)| *d = *v);
    if samples.is_empty() {
        1
    } else {
        0
    }
}

unsafe extern "C" fn host_apply_effect(ctx: *mut c_void, kind: *const c_char, json: *const c_char, img: *mut Image) -> i32 {
    let s = state(ctx);
    if kind.is_null() || img.is_null() || (*img).rgba.is_null() {
        return 1;
    }
    let kind = text(kind);
    // プラグインから別のプラグインを呼ぶと、直列化のロックで止まることがあるので組み込みに限る
    if super::is_plugin_kind(&kind) {
        return 1;
    }
    let json = text(json);
    let params: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(&json) {
        Ok(serde_json::Value::Object(m)) => m,
        _ if json.trim().is_empty() => Default::default(),
        _ => return 1,
    };
    let (w, h) = ((*img).width as usize, (*img).height as usize);
    if w == 0 || h == 0 {
        return 1;
    }
    let spec = super::hooks::FilterEffect { kind: kind.clone(), params };
    let scale = (s.ctx.scale as f64).max(1e-6);
    let Some(fx) = super::hooks::build_effect(&spec, w as f64 / scale, h as f64 / scale) else { return 1 };
    let p = fx.eval(s.ctx.time);
    let input = std::slice::from_raw_parts((*img).rgba, w * h * 4);
    let buf = FrameBuf { width: w, height: h, data: input.to_vec() };
    let result = crate::effects::apply(&kind, &p, buf, s.ctx);
    if result.width == w && result.height == h {
        std::slice::from_raw_parts_mut((*img).rgba, w * h * 4).copy_from_slice(&result.data);
        0
    } else {
        s.hand_out(result, img)
    }
}

unsafe extern "C" fn host_set_output(ctx: *mut c_void, w: u32, h: u32, ox: i32, oy: i32, out: *mut Image) -> i32 {
    let s = state(ctx);
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || out.is_null() {
        return 1;
    }
    let mut buf = FrameBuf::new(w as usize, h as usize);
    out.write(Image { rgba: buf.data.as_mut_ptr(), width: w, height: h });
    *s.output.borrow_mut() = Some((buf, [ox, oy]));
    0
}

unsafe extern "C" fn host_mask_count(ctx: *mut c_void) -> u32 {
    state(ctx).ctx.masks.len() as u32
}

/// index 番目のマスクを折れ線にして、(x, y) の組を out に最大 cap 組書く。戻り値は全体の点の数（cap より大きければ足りない）。
unsafe extern "C" fn host_mask_points(ctx: *mut c_void, index: u32, out: *mut f64, cap: u32, closed: *mut u32) -> u32 {
    let s = state(ctx);
    let Some(path) = s.ctx.masks.get(index as usize) else { return 0 };
    let line = crate::path::flatten(path, 0.25);
    if !closed.is_null() {
        closed.write(line.closed as u32);
    }
    if !out.is_null() {
        let dst = std::slice::from_raw_parts_mut(out, cap as usize * 2);
        for (i, p) in line.pts.iter().take(cap as usize).enumerate() {
            dst[i * 2] = p[0];
            dst[i * 2 + 1] = p[1];
        }
    }
    line.pts.len() as u32
}

/// レイヤーが入っているコンポの大きさ（コンポの画素）。
unsafe extern "C" fn host_comp_size(ctx: *mut c_void, w: *mut u32, h: *mut u32) {
    let [cw, ch] = state(ctx).ctx.comp_size;
    if !w.is_null() {
        w.write(cw);
    }
    if !h.is_null() {
        h.write(ch);
    }
}

/// index 番目のパラメータを、レイヤーのイン点から layer_time 秒の時刻で評価して out[4] に書く（render に渡す値と同じ形）。
/// 文字列・レイヤーのパラメータは 0 を書く。時間で変わるカーブ（時間制御）を読むのに使う。
unsafe extern "C" fn host_param_at(ctx: *mut c_void, index: u32, layer_time: f64, out: *mut f64) -> i32 {
    let s = state(ctx);
    let Some(param) = s.names.get(index as usize) else { return 1 };
    if out.is_null() || !layer_time.is_finite() {
        return 1;
    }
    let at = s.ctx.time + znd_core::Time::from_seconds(layer_time - s.ctx.layer_time);
    let p = (s.ctx.eval_at)(at);
    let color = |c: [f64; 4]| {
        let l = s.ctx.cx.rgb([c[0] as f32, c[1] as f32, c[2] as f32]);
        [l[0] as f64, l[1] as f64, l[2] as f64, c[3]]
    };
    let (values, _) = pack(std::slice::from_ref(param), &p, s.ctx.scale as f64, &color);
    std::slice::from_raw_parts_mut(out, 4).copy_from_slice(&values[..4]);
    0
}

impl Effect for NativeEffect {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn apply(&self, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
        if self.audio_only {
            return buf;
        }
        let s = ctx.scale as f64;
        let color = |c: [f64; 4]| {
            let l = ctx.cx.rgb([c[0] as f32, c[1] as f32, c[2] as f32]);
            [l[0] as f64, l[1] as f64, l[2] as f64, c[3]]
        };
        let (values, strings) = pack(&self.params, p, s, &color);
        let string_ptrs: Vec<*const c_char> = strings.iter().map(|s| s.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())).collect();
        let host_state = HostState {
            ctx,
            params: p,
            names: &self.params,
            keep: RefCell::default(),
            output: RefCell::default(),
            size: (buf.width, buf.height),
        };
        let host = Host {
            ctx: &host_state as *const HostState as *mut c_void,
            frame_at: host_frame_at,
            layer_image: host_layer_image,
            audio: host_audio,
            apply_effect: host_apply_effect,
            set_output: host_set_output,
            mask_count: host_mask_count,
            mask_points: host_mask_points,
            comp_size: host_comp_size,
            param_at: host_param_at,
            backdrop: host_backdrop,
            layer_source: host_layer_source,
        };
        let abi3 = self.lib.abi >= 3;
        let args = RenderArgs {
            rgba: buf.data.as_mut_ptr(),
            width: buf.width as u32,
            height: buf.height as u32,
            time: ctx.time.seconds(),
            fps: ctx.frame_rate.fps(),
            scale: s,
            seed: ctx.seed,
            param_count: self.params.len() as u32,
            values: values.as_ptr(),
            strings: if abi3 { string_ptrs.as_ptr() } else { std::ptr::null() },
            layer_time: ctx.layer_time,
            layer_duration: ctx.layer_duration,
            host: if abi3 { &host } else { std::ptr::null() },
        };
        // 失敗したときに元の画像へ戻せるよう、控えを取っておく
        let backup = buf.data.clone();
        let _guard = (!self.thread_safe).then(|| self.lib.lock.lock().unwrap_or_else(|e| e.into_inner()));
        // SAFETY: バッファは width*height*4 要素あり、呼び出しの間は排他的に借用している。values は param_count*4 要素。
        let status = unsafe { (self.lib.render)(self.index, &args) };
        let output = host_state.output.borrow_mut().take();
        if status != 0 {
            buf.data = backup;
            return buf;
        }
        match output {
            Some((out, [ox, oy])) if out.data.iter().all(|v| v.is_finite()) => {
                // 出力の左上がずれた分を、レイヤー上の置き場所に反映する
                let o = ctx.offset.get();
                ctx.offset.set([o[0] + ox as f64, o[1] + oy as f64]);
                out
            }
            Some(_) => {
                buf.data = backup;
                buf
            }
            None => {
                if buf.data.iter().any(|v| !v.is_finite()) {
                    buf.data = backup;
                }
                buf
            }
        }
    }
}

impl AudioProcessor for NativeAudio {
    fn process(&self, p: &EvalParams, block: AudioBlock) {
        let Some(audio) = self.lib.audio else { return };
        let (values, strings) = pack(&self.params, p, 1.0, &|c| c);
        let string_ptrs: Vec<*const c_char> = strings.iter().map(|s| s.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())).collect();
        let backup = block.samples.to_vec();
        let args = AudioArgs {
            samples: block.samples.as_mut_ptr(),
            frames: (block.samples.len() / 2) as u32,
            sample_rate: block.sample_rate,
            start: block.start,
            time: block.time.seconds(),
            instance: block.instance,
            seed: block.seed,
            param_count: self.params.len() as u32,
            values: values.as_ptr(),
            strings: string_ptrs.as_ptr(),
            length: block.length,
        };
        let _guard = (!self.thread_safe).then(|| self.lib.lock.lock().unwrap_or_else(|e| e.into_inner()));
        // SAFETY: samples は frames*2 要素で、呼び出しの間は排他的に借用している。
        let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { audio(self.index, &args) })).unwrap_or(1);
        if status != 0 {
            block.samples.copy_from_slice(&backup);
        }
    }
}
