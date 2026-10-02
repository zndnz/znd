//! znd ネイティブプラグイン（`sdk/znd_plugin.h`、ABI 2）の読み込みと実行。

use super::{Effect, PluginApi, PluginInfo};
use crate::effects::FxContext;
use crate::frame::FrameBuf;
use std::ffi::{c_char, CStr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use znd_core::effect::{register_effect, DynEffect, DynPd, EvalParams};

pub const ABI_VERSION: u32 = 2;

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
}

const PARAM_FLOAT: u32 = 0;
const PARAM_BOOL: u32 = 1;
const PARAM_CHOICE: u32 = 2;
const PARAM_COLOR: u32 = 3;
const PARAM_POINT: u32 = 4;
const PARAM_PIXELS: u32 = 1;
const EFFECT_TIME_DEPENDENT: u32 = 1;
const EFFECT_THREAD_SAFE: u32 = 2;

type DescribeFn = unsafe extern "C" fn() -> *const PluginDesc;
type RenderFn = unsafe extern "C" fn(u32, *const RenderArgs) -> i32;

struct Lib {
    // ライブラリを保持し続ける（関数ポインタの寿命を保証する）
    _lib: libloading::Library,
    render: RenderFn,
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
    params: Vec<Param>,
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
        let desc = describe();
        if desc.is_null() {
            return Err("プラグインの定義を取得できません".into());
        }
        let desc = &*desc;
        if desc.abi_version != ABI_VERSION {
            return Err(format!("ABI バージョン {} は未対応です（{ABI_VERSION} が必要）", desc.abi_version));
        }
        if desc.effect_count > 1024 || (desc.effect_count > 0 && desc.effects.is_null()) {
            return Err("エフェクトの定義が不正です".into());
        }
        let (vendor, version, license) = (text(desc.vendor), text(desc.version), text(desc.license));
        let descs = std::slice::from_raw_parts(desc.effects, desc.effect_count as usize);
        let lib = Arc::new(Lib { _lib: lib, render, lock: Mutex::new(()) });
        let mut out: Vec<Arc<dyn Effect>> = Vec::new();
        for (index, e) in descs.iter().enumerate() {
            let id = text(e.id);
            if !valid_id(&id) {
                return Err(format!("エフェクトの ID「{id}」が不正です（英数字と . _ - のみ）"));
            }
            if e.param_count > 256 || (e.param_count > 0 && e.params.is_null()) {
                return Err(format!("{id}: パラメータの定義が不正です"));
            }
            let pdescs = std::slice::from_raw_parts(e.params, e.param_count as usize);
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
                    other => return Err(format!("{id}.{name}: 未対応のパラメータ種別です（{other}）")),
                };
                params.push(Param { name: name.clone(), kind: p.kind, pixels: p.flags & PARAM_PIXELS != 0 });
                defs.push((name, label, pd));
            }
            let kind = format!("plugin:{id}");
            let label = Some(text(e.name)).filter(|l| !l.is_empty()).unwrap_or_else(|| id.clone());
            let category = Some(text(e.category)).filter(|c| !c.is_empty()).unwrap_or_else(|| "プラグイン".into());
            register_effect(DynEffect {
                kind: kind.clone(),
                label: label.clone(),
                category: category.clone(),
                time_dependent: e.flags & EFFECT_TIME_DEPENDENT != 0,
                params: defs,
            });
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
                thread_safe: e.flags & EFFECT_THREAD_SAFE != 0,
            }));
        }
        Ok(out)
    }
}

impl Effect for NativeEffect {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn apply(&self, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
        let s = ctx.scale as f64;
        let mut values = vec![0.0f64; self.params.len() * 4];
        for (param, v) in self.params.iter().zip(values.chunks_exact_mut(4)) {
            match param.kind {
                PARAM_FLOAT => v[0] = p.f(&param.name) * if param.pixels { s } else { 1.0 },
                PARAM_BOOL => v[0] = p.b(&param.name) as u8 as f64,
                PARAM_CHOICE => v[0] = p.i(&param.name) as f64,
                PARAM_COLOR => {
                    let c = p.c(&param.name);
                    let l = ctx.cx.rgb([c[0] as f32, c[1] as f32, c[2] as f32]);
                    v.copy_from_slice(&[l[0] as f64, l[1] as f64, l[2] as f64, c[3]]);
                }
                PARAM_POINT => {
                    let pt = p.p(&param.name);
                    (v[0], v[1]) = (pt[0] * s, pt[1] * s);
                }
                _ => {}
            }
        }
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
        };
        // 失敗したときに元の画像へ戻せるよう、控えを取っておく
        let backup = buf.data.clone();
        let _guard = (!self.thread_safe).then(|| self.lib.lock.lock().unwrap_or_else(|e| e.into_inner()));
        // SAFETY: バッファは width*height*4 要素あり、呼び出しの間は排他的に借用している。values は param_count*4 要素。
        let status = unsafe { (self.lib.render)(self.index, &args) };
        if status != 0 || buf.data.iter().any(|v| !v.is_finite()) {
            buf.data = backup;
        }
        buf
    }
}
