//! OpenFX 画像エフェクトのホスト（OFX 1.4 の Filter / General コンテキスト）。
//!
//! 対応範囲:
//! - スイート: Property / ImageEffect / Parameter / Memory / MultiThread / Message（V1・V2）
//! - 画像: RGBA・32bit float・プリマルチプライド。入力クリップは「Source」1 本
//! - パラメータ: Double / Integer / Boolean / Choice / RGB / RGBA / Double2D / Integer2D / Double3D / Integer3D / String
//!   （Group / Page / PushButton は無視、Custom は既定値のまま）
//! - 別時刻の入力（時間系プラグイン）は、レイヤー自身の別時刻の絵を渡す
//!
//! 未対応: オーバーレイ（画面上の操作 UI）、複数入力、タイル描画、OpenGL 描画、パラメトリックパラメータ。
//! これらを必須とするプラグインは読み込み時または描画時に失敗し、元の画像のまま続ける。
//!
//! OFX の画像は下の行が先（Y 上向き）なので、受け渡しのたびに上下を入れ替える。

use super::{Effect, PluginApi, PluginInfo};
use crate::effects::FxContext;
use crate::frame::FrameBuf;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_double, c_int, c_uint, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use znd_core::effect::{register_effect, DynEffect, DynPd, EvalParams, EvalValue};
use znd_core::Time;

type Status = c_int;
const OK: Status = 0;
const FAILED: Status = 1;
const ERR_UNKNOWN: Status = 3;
const ERR_UNSUPPORTED: Status = 5;
const ERR_MEMORY: Status = 8;
const ERR_BAD_HANDLE: Status = 9;
const ERR_BAD_INDEX: Status = 10;
const ERR_VALUE: Status = 11;
const REPLY_DEFAULT: Status = 14;

// ---------- プロパティセット ----------

#[derive(Clone, Debug)]
enum Val {
    I(c_int),
    D(f64),
    S(CString),
    P(*mut c_void),
}

#[derive(Default)]
pub(crate) struct PropSet {
    map: HashMap<String, Vec<Val>>,
}

fn cs(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl PropSet {
    fn set_s(&mut self, name: &str, v: &[&str]) -> &mut Self {
        self.map.insert(name.into(), v.iter().map(|s| Val::S(cs(s))).collect());
        self
    }
    fn set_i(&mut self, name: &str, v: &[c_int]) -> &mut Self {
        self.map.insert(name.into(), v.iter().map(|i| Val::I(*i)).collect());
        self
    }
    fn set_d(&mut self, name: &str, v: &[f64]) -> &mut Self {
        self.map.insert(name.into(), v.iter().map(|d| Val::D(*d)).collect());
        self
    }
    fn set_p(&mut self, name: &str, v: *mut c_void) -> &mut Self {
        self.map.insert(name.into(), vec![Val::P(v)]);
        self
    }
    fn strings(&self, name: &str) -> Vec<String> {
        self.map
            .get(name)
            .map(|v| v.iter().filter_map(|x| if let Val::S(s) = x { Some(s.to_string_lossy().into_owned()) } else { None }).collect())
            .unwrap_or_default()
    }
    fn string(&self, name: &str) -> String {
        self.strings(name).into_iter().next().unwrap_or_default()
    }
    fn doubles(&self, name: &str) -> Vec<f64> {
        self.map
            .get(name)
            .map(|v| {
                v.iter()
                    .filter_map(|x| match x {
                        Val::D(d) => Some(*d),
                        Val::I(i) => Some(*i as f64),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn int(&self, name: &str) -> c_int {
        self.doubles(name).first().map(|d| *d as c_int).unwrap_or(0)
    }
}

/// 環境変数 `ZND_OFX_TRACE` を設定すると、プラグインが読もうとして見つからなかったプロパティなどを stderr に出す
/// （プラグインが動かないときの調査用）。
fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("ZND_OFX_TRACE").is_some())
}

unsafe fn name_of(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

unsafe fn prop_set(props: *mut PropSet, name: *const c_char, index: c_int, v: Val) -> Status {
    let (Some(ps), Some(name)) = (props.as_mut(), name_of(name)) else { return ERR_BAD_HANDLE };
    if !(0..65536).contains(&index) {
        return ERR_BAD_INDEX;
    }
    let slot = ps.map.entry(name).or_default();
    while slot.len() <= index as usize {
        // 途中の要素は同じ型の既定値で埋める
        slot.push(match &v {
            Val::I(_) => Val::I(0),
            Val::D(_) => Val::D(0.0),
            Val::S(_) => Val::S(CString::default()),
            Val::P(_) => Val::P(std::ptr::null_mut()),
        });
    }
    slot[index as usize] = v;
    OK
}

unsafe fn prop_get<'a>(props: *mut PropSet, name: *const c_char, index: c_int) -> Result<&'a Val, Status> {
    let (Some(ps), Some(name)) = (props.as_ref(), name_of(name)) else { return Err(ERR_BAD_HANDLE) };
    let found =
        ps.map.get(&name).ok_or(ERR_UNKNOWN).and_then(|slot| slot.get(index.max(0) as usize).filter(|_| index >= 0).ok_or(ERR_BAD_INDEX));
    if found.is_err() && trace() {
        eprintln!("OpenFX: プロパティ {name}[{index}] は未設定です");
    }
    found
}

unsafe fn type_mismatch(name: *const c_char) -> Status {
    if trace() {
        eprintln!("OpenFX: プロパティ {} の型が合いません", name_of(name).unwrap_or_default());
    }
    ERR_VALUE
}

unsafe extern "C" fn prop_set_pointer(p: *mut PropSet, n: *const c_char, i: c_int, v: *mut c_void) -> Status {
    prop_set(p, n, i, Val::P(v))
}
unsafe extern "C" fn prop_set_string(p: *mut PropSet, n: *const c_char, i: c_int, v: *const c_char) -> Status {
    let s = if v.is_null() { CString::default() } else { CStr::from_ptr(v).to_owned() };
    prop_set(p, n, i, Val::S(s))
}
unsafe extern "C" fn prop_set_double(p: *mut PropSet, n: *const c_char, i: c_int, v: c_double) -> Status {
    prop_set(p, n, i, Val::D(v))
}
unsafe extern "C" fn prop_set_int(p: *mut PropSet, n: *const c_char, i: c_int, v: c_int) -> Status {
    prop_set(p, n, i, Val::I(v))
}
unsafe extern "C" fn prop_set_pointer_n(p: *mut PropSet, n: *const c_char, count: c_int, v: *const *mut c_void) -> Status {
    (0..count.max(0)).map(|i| prop_set(p, n, i, Val::P(*v.add(i as usize)))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_set_string_n(p: *mut PropSet, n: *const c_char, count: c_int, v: *const *const c_char) -> Status {
    (0..count.max(0)).map(|i| prop_set_string(p, n, i, *v.add(i as usize))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_set_double_n(p: *mut PropSet, n: *const c_char, count: c_int, v: *const c_double) -> Status {
    (0..count.max(0)).map(|i| prop_set(p, n, i, Val::D(*v.add(i as usize)))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_set_int_n(p: *mut PropSet, n: *const c_char, count: c_int, v: *const c_int) -> Status {
    (0..count.max(0)).map(|i| prop_set(p, n, i, Val::I(*v.add(i as usize)))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_get_pointer(p: *mut PropSet, n: *const c_char, i: c_int, out: *mut *mut c_void) -> Status {
    match prop_get(p, n, i) {
        Ok(Val::P(v)) => {
            *out = *v;
            OK
        }
        Ok(_) => type_mismatch(n),
        Err(e) => e,
    }
}
unsafe extern "C" fn prop_get_string(p: *mut PropSet, n: *const c_char, i: c_int, out: *mut *const c_char) -> Status {
    match prop_get(p, n, i) {
        Ok(Val::S(v)) => {
            *out = v.as_ptr();
            OK
        }
        Ok(_) => type_mismatch(n),
        Err(e) => e,
    }
}
unsafe extern "C" fn prop_get_double(p: *mut PropSet, n: *const c_char, i: c_int, out: *mut c_double) -> Status {
    match prop_get(p, n, i) {
        Ok(Val::D(v)) => {
            *out = *v;
            OK
        }
        Ok(Val::I(v)) => {
            *out = *v as f64;
            OK
        }
        Ok(_) => type_mismatch(n),
        Err(e) => e,
    }
}
unsafe extern "C" fn prop_get_int(p: *mut PropSet, n: *const c_char, i: c_int, out: *mut c_int) -> Status {
    match prop_get(p, n, i) {
        Ok(Val::I(v)) => {
            *out = *v;
            OK
        }
        Ok(Val::D(v)) => {
            *out = *v as c_int;
            OK
        }
        Ok(_) => type_mismatch(n),
        Err(e) => e,
    }
}
unsafe extern "C" fn prop_get_pointer_n(p: *mut PropSet, n: *const c_char, count: c_int, out: *mut *mut c_void) -> Status {
    (0..count.max(0)).map(|i| prop_get_pointer(p, n, i, out.add(i as usize))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_get_string_n(p: *mut PropSet, n: *const c_char, count: c_int, out: *mut *const c_char) -> Status {
    (0..count.max(0)).map(|i| prop_get_string(p, n, i, out.add(i as usize))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_get_double_n(p: *mut PropSet, n: *const c_char, count: c_int, out: *mut c_double) -> Status {
    (0..count.max(0)).map(|i| prop_get_double(p, n, i, out.add(i as usize))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_get_int_n(p: *mut PropSet, n: *const c_char, count: c_int, out: *mut c_int) -> Status {
    (0..count.max(0)).map(|i| prop_get_int(p, n, i, out.add(i as usize))).find(|s| *s != OK).unwrap_or(OK)
}
unsafe extern "C" fn prop_reset(p: *mut PropSet, n: *const c_char) -> Status {
    let (Some(ps), Some(name)) = (p.as_mut(), name_of(n)) else { return ERR_BAD_HANDLE };
    ps.map.remove(&name);
    OK
}
unsafe extern "C" fn prop_get_dimension(p: *mut PropSet, n: *const c_char, out: *mut c_int) -> Status {
    let (Some(ps), Some(name)) = (p.as_ref(), name_of(n)) else { return ERR_BAD_HANDLE };
    match ps.map.get(&name) {
        Some(v) => {
            *out = v.len() as c_int;
            OK
        }
        None => {
            // 値が 1 つも入っていない可変長プロパティ（ページの子など）は「0 個」として答える
            *out = 0;
            OK
        }
    }
}

#[repr(C)]
struct PropertySuite {
    set_pointer: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut c_void) -> Status,
    set_string: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *const c_char) -> Status,
    set_double: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, c_double) -> Status,
    set_int: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, c_int) -> Status,
    set_pointer_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *const *mut c_void) -> Status,
    set_string_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *const *const c_char) -> Status,
    set_double_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *const c_double) -> Status,
    set_int_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *const c_int) -> Status,
    get_pointer: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut *mut c_void) -> Status,
    get_string: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut *const c_char) -> Status,
    get_double: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut c_double) -> Status,
    get_int: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut c_int) -> Status,
    get_pointer_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut *mut c_void) -> Status,
    get_string_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut *const c_char) -> Status,
    get_double_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut c_double) -> Status,
    get_int_n: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int, *mut c_int) -> Status,
    reset: unsafe extern "C" fn(*mut PropSet, *const c_char) -> Status,
    get_dimension: unsafe extern "C" fn(*mut PropSet, *const c_char, *mut c_int) -> Status,
}

static PROPERTY_SUITE: PropertySuite = PropertySuite {
    set_pointer: prop_set_pointer,
    set_string: prop_set_string,
    set_double: prop_set_double,
    set_int: prop_set_int,
    set_pointer_n: prop_set_pointer_n,
    set_string_n: prop_set_string_n,
    set_double_n: prop_set_double_n,
    set_int_n: prop_set_int_n,
    get_pointer: prop_get_pointer,
    get_string: prop_get_string,
    get_double: prop_get_double,
    get_int: prop_get_int,
    get_pointer_n: prop_get_pointer_n,
    get_string_n: prop_get_string_n,
    get_double_n: prop_get_double_n,
    get_int_n: prop_get_int_n,
    reset: prop_reset,
    get_dimension: prop_get_dimension,
};

// ---------- エフェクト・クリップ・パラメータ ----------

// C のシム（ofx_shim.c）と同じ番号
const K_DOUBLE: c_int = 0;
const K_INT: c_int = 1;
const K_RGBA: c_int = 2;
const K_RGB: c_int = 3;
const K_DOUBLE2D: c_int = 4;
const K_INT2D: c_int = 5;
const K_DOUBLE3D: c_int = 6;
const K_INT3D: c_int = 7;
const K_STRING: c_int = 8;
/// 値を持たない（グループ・ページ・ボタン）か、対応していない種類。
const K_NONE: c_int = -1;

const PARAM_MAGIC: u32 = 0x5a4e_4450;

struct ParamObj {
    magic: u32,
    kind: c_int,
    name: String,
    ofx_type: String,
    props: PropSet,
    value: [f64; 4],
    text: CString,
    /// 別時刻の値を返すときの置き場（返したポインタを呼び出しの間だけ生かす）。
    scratch: CString,
}

struct ClipObj {
    name: String,
    props: PropSet,
}

struct EffectObj {
    props: PropSet,
    param_props: PropSet,
    params: Vec<Box<ParamObj>>,
    clips: Vec<Box<ClipObj>>,
}

fn param_kind(ofx_type: &str) -> c_int {
    match ofx_type {
        "OfxParamTypeDouble" => K_DOUBLE,
        "OfxParamTypeInteger" | "OfxParamTypeBoolean" | "OfxParamTypeChoice" => K_INT,
        "OfxParamTypeRGBA" => K_RGBA,
        "OfxParamTypeRGB" => K_RGB,
        "OfxParamTypeDouble2D" => K_DOUBLE2D,
        "OfxParamTypeInteger2D" => K_INT2D,
        "OfxParamTypeDouble3D" => K_DOUBLE3D,
        "OfxParamTypeInteger3D" => K_INT3D,
        "OfxParamTypeString" | "OfxParamTypeCustom" => K_STRING,
        _ => K_NONE,
    }
}

fn components(kind: c_int) -> usize {
    match kind {
        K_RGBA => 4,
        K_RGB | K_DOUBLE3D | K_INT3D => 3,
        K_DOUBLE2D | K_INT2D => 2,
        _ => 1,
    }
}

fn new_param(ofx_type: &str, name: &str) -> Box<ParamObj> {
    let kind = param_kind(ofx_type);
    let mut props = PropSet::default();
    props
        .set_s("OfxPropType", &["OfxTypeParameter"])
        .set_s("OfxPropName", &[name])
        .set_s("OfxPropLabel", &[name])
        .set_s("OfxPropShortLabel", &[name])
        .set_s("OfxPropLongLabel", &[name])
        .set_s("OfxParamPropType", &[ofx_type])
        .set_s("OfxParamPropScriptName", &[name])
        .set_s("OfxParamPropHint", &[""])
        .set_s("OfxParamPropParent", &[""])
        .set_i("OfxParamPropSecret", &[0])
        .set_i("OfxParamPropEnabled", &[1])
        .set_i("OfxParamPropAnimates", &[1])
        .set_i("OfxParamPropPersistant", &[1])
        .set_i("OfxParamPropEvaluateOnChange", &[1])
        .set_i("OfxParamPropPluginMayWrite", &[0])
        .set_s("OfxParamPropCacheInvalidation", &["OfxParamInvalidateValueChange"])
        .set_i("OfxParamPropCanUndo", &[1])
        .set_i("OfxParamPropIsAnimating", &[0])
        .set_i("OfxParamPropIsAutoKeying", &[0])
        .set_p("OfxParamPropDataPtr", std::ptr::null_mut());
    let n = components(kind);
    match kind {
        K_DOUBLE | K_RGBA | K_RGB | K_DOUBLE2D | K_DOUBLE3D => {
            let colour = matches!(kind, K_RGBA | K_RGB);
            let (lo, hi) = if colour { (0.0, 1.0) } else { (-f64::MAX, f64::MAX) };
            props
                .set_d("OfxParamPropDefault", &vec![0.0; n])
                .set_d("OfxParamPropMin", &vec![lo; n])
                .set_d("OfxParamPropMax", &vec![hi; n])
                .set_d("OfxParamPropDisplayMin", &vec![lo; n])
                .set_d("OfxParamPropDisplayMax", &vec![hi; n])
                .set_d("OfxParamPropIncrement", &[1.0])
                .set_i("OfxParamPropDigits", &[2])
                .set_s("OfxParamPropDoubleType", &["OfxParamDoubleTypePlain"])
                .set_s("OfxParamPropDefaultCoordinateSystem", &["OfxParamCoordinatesCanonical"]);
        }
        K_INT | K_INT2D | K_INT3D => {
            let (lo, hi) = if ofx_type == "OfxParamTypeBoolean" { (0, 1) } else { (c_int::MIN, c_int::MAX) };
            props
                .set_i("OfxParamPropDefault", &vec![0; n])
                .set_i("OfxParamPropMin", &vec![lo; n])
                .set_i("OfxParamPropMax", &vec![hi; n])
                .set_i("OfxParamPropDisplayMin", &vec![lo; n])
                .set_i("OfxParamPropDisplayMax", &vec![hi; n]);
        }
        K_STRING => {
            props.set_s("OfxParamPropDefault", &[""]).set_s("OfxParamPropStringMode", &["OfxParamStringIsSingleLine"]);
        }
        _ => {}
    }
    Box::new(ParamObj {
        magic: PARAM_MAGIC,
        kind,
        name: name.into(),
        ofx_type: ofx_type.into(),
        props,
        value: [0.0; 4],
        text: CString::default(),
        scratch: CString::default(),
    })
}

fn new_effect(type_name: &str, path: &Path) -> Box<EffectObj> {
    let mut props = PropSet::default();
    props
        .set_s("OfxPropType", &[type_name])
        .set_s("OfxPropLabel", &[""])
        .set_s("OfxPropShortLabel", &[""])
        .set_s("OfxPropLongLabel", &[""])
        .set_s("OfxImageEffectPluginPropGrouping", &[""])
        .set_s("OfxPluginPropFilePath", &[&path.to_string_lossy()])
        .set_s("OfxImageEffectPluginRenderThreadSafety", &["OfxImageEffectRenderInstanceSafe"])
        .set_i("OfxImageEffectPluginPropSingleInstance", &[0])
        .set_i("OfxImageEffectPluginPropHostFrameThreading", &[0])
        .set_i("OfxImageEffectPropSupportsMultiResolution", &[1])
        .set_i("OfxImageEffectPropSupportsTiles", &[1])
        .set_i("OfxImageEffectPropTemporalClipAccess", &[0])
        .set_i("OfxImageEffectPluginPropFieldRenderTwiceAlways", &[1])
        .set_i("OfxImageEffectPropMultipleClipDepths", &[0])
        .set_i("OfxImageEffectPropSupportsMultipleClipPARs", &[0])
        .set_i("OfxImageEffectPropClipPreferencesSlaveParam", &[0])
        .set_p("OfxImageEffectPluginPropOverlayInteractV1", std::ptr::null_mut())
        .set_p("OfxPropInstanceData", std::ptr::null_mut());
    props.map.insert("OfxImageEffectPropSupportedContexts".into(), Vec::new());
    props.map.insert("OfxImageEffectPropSupportedPixelDepths".into(), Vec::new());
    let mut param_props = PropSet::default();
    param_props.set_s("OfxPropType", &["OfxTypeParameterSet"]);
    Box::new(EffectObj { props, param_props, params: Vec::new(), clips: Vec::new() })
}

fn new_clip(name: &str) -> Box<ClipObj> {
    let mut props = PropSet::default();
    props
        .set_s("OfxPropType", &["OfxTypeClip"])
        .set_s("OfxPropName", &[name])
        .set_s("OfxPropLabel", &[name])
        .set_s("OfxPropShortLabel", &[name])
        .set_s("OfxPropLongLabel", &[name])
        .set_i("OfxImageEffectPropTemporalClipAccess", &[0])
        .set_i("OfxImageClipPropOptional", &[0])
        .set_i("OfxImageClipPropIsMask", &[0])
        .set_i("OfxImageEffectPropSupportsTiles", &[1])
        .set_s("OfxImageClipPropFieldExtraction", &["OfxFieldDoubled"]);
    props.map.insert("OfxImageEffectPropSupportedComponents".into(), Vec::new());
    Box::new(ClipObj { name: name.into(), props })
}

/// プラグインとやり取りする画素の形式。float に対応していないプラグインには整数で渡す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    Float,
    Short,
    Byte,
}

impl Depth {
    fn name(self) -> &'static str {
        match self {
            Depth::Float => "OfxBitDepthFloat",
            Depth::Short => "OfxBitDepthShort",
            Depth::Byte => "OfxBitDepthByte",
        }
    }
    fn bytes(self) -> usize {
        match self {
            Depth::Float => 4,
            Depth::Short => 2,
            Depth::Byte => 1,
        }
    }
}

/// 8 バイト境界に揃えた画素バッファ（プラグインは float* や short* として読む）。
struct Pixels {
    store: Vec<u64>,
}

impl Pixels {
    fn zeroed(w: usize, h: usize, depth: Depth) -> Pixels {
        Pixels { store: vec![0u64; (w * h * 4 * depth.bytes()).div_ceil(8)] }
    }
    fn ptr(&mut self) -> *mut c_void {
        self.store.as_mut_ptr() as *mut c_void
    }
    /// リニア・プリマルチプライドの画像（上が先）から、上下を入れ替えて作る。
    /// 整数形式は表示値（sRGB 符号化）で渡す（8/16bit の画像は表示値で扱うのが通例のため）。
    fn from_frame(src: &[f32], w: usize, h: usize, depth: Depth) -> Pixels {
        let mut out = Pixels::zeroed(w, h, depth);
        let row = w * 4;
        let base = out.store.as_mut_ptr();
        for y in 0..h {
            let line = &src[y * row..(y + 1) * row];
            let dy = h - 1 - y;
            // SAFETY: store は w*h*4*bytes バイト以上あり、行 dy はその範囲内。
            unsafe {
                match depth {
                    Depth::Float => std::slice::from_raw_parts_mut((base as *mut f32).add(dy * row), row).copy_from_slice(line),
                    Depth::Short => {
                        let d = std::slice::from_raw_parts_mut((base as *mut u16).add(dy * row), row);
                        for (o, px) in d.chunks_exact_mut(4).zip(line.chunks_exact(4)) {
                            let q = encode_display(px);
                            for k in 0..4 {
                                o[k] = (q[k] * 65535.0).round() as u16;
                            }
                        }
                    }
                    Depth::Byte => {
                        let d = std::slice::from_raw_parts_mut((base as *mut u8).add(dy * row), row);
                        for (o, px) in d.chunks_exact_mut(4).zip(line.chunks_exact(4)) {
                            let q = encode_display(px);
                            for k in 0..4 {
                                o[k] = (q[k] * 255.0).round() as u8;
                            }
                        }
                    }
                }
            }
        }
        out
    }
    /// `from_frame` の逆。
    fn to_frame(&self, w: usize, h: usize, depth: Depth) -> Vec<f32> {
        let row = w * 4;
        let mut out = vec![0.0f32; row * h];
        let base = self.store.as_ptr();
        for y in 0..h {
            let line = &mut out[y * row..(y + 1) * row];
            let sy = h - 1 - y;
            // SAFETY: from_frame と同じ範囲。
            unsafe {
                match depth {
                    Depth::Float => line.copy_from_slice(std::slice::from_raw_parts((base as *const f32).add(sy * row), row)),
                    Depth::Short => {
                        let d = std::slice::from_raw_parts((base as *const u16).add(sy * row), row);
                        for (o, px) in line.chunks_exact_mut(4).zip(d.chunks_exact(4)) {
                            o.copy_from_slice(&decode_display([0, 1, 2, 3].map(|k| px[k] as f32 / 65535.0)));
                        }
                    }
                    Depth::Byte => {
                        let d = std::slice::from_raw_parts((base as *const u8).add(sy * row), row);
                        for (o, px) in line.chunks_exact_mut(4).zip(d.chunks_exact(4)) {
                            o.copy_from_slice(&decode_display([0, 1, 2, 3].map(|k| px[k] as f32 / 255.0)));
                        }
                    }
                }
            }
        }
        out
    }
}

/// リニア・プリマルチプライド → 表示値・プリマルチプライド（0..1）。
fn encode_display(px: &[f32]) -> [f32; 4] {
    let a = px[3].clamp(0.0, 1.0);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let c = |v: f32| crate::color::linear_to_srgb((v / a).clamp(0.0, 1.0)) * a;
    [c(px[0]), c(px[1]), c(px[2]), a]
}

fn decode_display(px: [f32; 4]) -> [f32; 4] {
    let a = px[3];
    if a <= 0.0 {
        return [0.0; 4];
    }
    let c = |v: f32| crate::color::srgb_to_linear((v / a).clamp(0.0, 1.0)) * a;
    [c(px[0]), c(px[1]), c(px[2]), a]
}

/// 描画中の状態（プラグインがクリップの画像や別時刻のパラメータを求めたときに使う）。
struct RenderCx {
    /// 上下を入れ替えた入力と出力。
    src: Pixels,
    dst: Pixels,
    depth: Depth,
    w: usize,
    h: usize,
    /// 現在時刻（フレーム単位）。
    time: f64,
    fps: f64,
    scale: f64,
    /// 別時刻の入力画像（上下入れ替え済み）を返す。
    fetch: *const (dyn Fn(f64) -> Option<Pixels> + 'static),
    /// 別時刻のパラメータ値を返す。
    eval: *const (dyn Fn(&ParamObj, f64) -> Option<([f64; 4], Option<CString>)> + 'static),
}

thread_local! {
    static RENDER: RefCell<Option<RenderCx>> = const { RefCell::new(None) };
    static THREAD_INDEX: Cell<c_uint> = const { Cell::new(0) };
    static SPAWNED: Cell<bool> = const { Cell::new(false) };
}

/// 1 枚の画像（プロパティセットが先頭にあり、そのポインタが画像ハンドルになる）。
#[repr(C)]
struct ImageObj {
    props: PropSet,
    _owned: Option<Pixels>,
}

unsafe extern "C" fn fx_get_property_set(effect: *mut EffectObj, out: *mut *mut PropSet) -> Status {
    let Some(e) = effect.as_mut() else { return ERR_BAD_HANDLE };
    *out = &mut e.props;
    OK
}

unsafe extern "C" fn fx_get_param_set(effect: *mut EffectObj, out: *mut *mut EffectObj) -> Status {
    if effect.is_null() {
        return ERR_BAD_HANDLE;
    }
    // パラメータセットのハンドルはエフェクトと同じものを使う
    *out = effect;
    OK
}

unsafe extern "C" fn fx_clip_define(effect: *mut EffectObj, name: *const c_char, out: *mut *mut PropSet) -> Status {
    let (Some(e), Some(name)) = (effect.as_mut(), name_of(name)) else { return ERR_BAD_HANDLE };
    if !e.clips.iter().any(|c| c.name == name) {
        e.clips.push(new_clip(&name));
    }
    let clip = e.clips.iter_mut().find(|c| c.name == name).expect("clip was just added");
    if !out.is_null() {
        *out = &mut clip.props;
    }
    OK
}

unsafe extern "C" fn fx_clip_get_handle(
    effect: *mut EffectObj,
    name: *const c_char,
    clip: *mut *mut ClipObj,
    props: *mut *mut PropSet,
) -> Status {
    let (Some(e), Some(name)) = (effect.as_mut(), name_of(name)) else { return ERR_BAD_HANDLE };
    let Some(c) = e.clips.iter_mut().find(|c| c.name == name) else { return ERR_BAD_HANDLE };
    if !clip.is_null() {
        *clip = &mut **c;
    }
    if !props.is_null() {
        *props = &mut c.props;
    }
    OK
}

unsafe extern "C" fn fx_clip_get_property_set(clip: *mut ClipObj, out: *mut *mut PropSet) -> Status {
    let Some(c) = clip.as_mut() else { return ERR_BAD_HANDLE };
    *out = &mut c.props;
    OK
}

unsafe extern "C" fn fx_clip_get_image(clip: *mut ClipObj, time: c_double, _region: *const c_double, out: *mut *mut PropSet) -> Status {
    let Some(c) = clip.as_ref() else { return ERR_BAD_HANDLE };
    if trace() {
        eprintln!("OpenFX: clipGetImage {} @ {time}", c.name);
    }
    RENDER.with(|r| {
        let mut r = r.borrow_mut();
        let Some(cx) = r.as_mut() else { return FAILED };
        let (w, h) = (cx.w, cx.h);
        let (data, owned): (*mut c_void, Option<Pixels>) = if c.name == "Output" {
            (cx.dst.ptr(), None)
        } else if c.name != "Source" {
            // 接続していない任意入力（マスク等）
            return FAILED;
        } else if (time - cx.time).abs() < 1e-6 {
            (cx.src.ptr(), None)
        } else {
            // 別時刻の入力
            match (*cx.fetch)(time) {
                Some(mut v) => (v.ptr(), Some(v)),
                None => return FAILED,
            }
        };
        let mut props = PropSet::default();
        props
            .set_s("OfxPropType", &["OfxTypeImage"])
            .set_p("OfxImagePropData", data)
            .set_i("OfxImagePropBounds", &[0, 0, w as c_int, h as c_int])
            .set_i("OfxImagePropRegionOfDefinition", &[0, 0, w as c_int, h as c_int])
            .set_i("OfxImagePropRowBytes", &[(w * 4 * cx.depth.bytes()) as c_int])
            .set_s("OfxImageEffectPropPixelDepth", &[cx.depth.name()])
            .set_s("OfxImageEffectPropComponents", &["OfxImageComponentRGBA"])
            .set_s("OfxImageEffectPropPreMultiplication", &["OfxImageAlphaPremultiplied"])
            .set_d("OfxImageEffectPropRenderScale", &[cx.scale, cx.scale])
            .set_d("OfxImagePropPixelAspectRatio", &[1.0])
            .set_s("OfxImagePropField", &["OfxFieldNone"])
            .set_s("OfxImagePropUniqueIdentifier", &[&format!("{}@{time}", c.name)]);
        let img = Box::into_raw(Box::new(ImageObj { props, _owned: owned }));
        *out = img as *mut PropSet;
        OK
    })
}

unsafe extern "C" fn fx_clip_release_image(image: *mut PropSet) -> Status {
    if image.is_null() {
        return ERR_BAD_HANDLE;
    }
    drop(Box::from_raw(image as *mut ImageObj));
    OK
}

unsafe extern "C" fn fx_clip_get_rod(clip: *mut ClipObj, _time: c_double, bounds: *mut c_double) -> Status {
    if clip.is_null() || bounds.is_null() {
        return ERR_BAD_HANDLE;
    }
    RENDER.with(|r| match r.borrow().as_ref() {
        Some(cx) => {
            // 正準座標（描画スケールを掛ける前）
            let b = [0.0, 0.0, cx.w as f64 / cx.scale, cx.h as f64 / cx.scale];
            std::ptr::copy_nonoverlapping(b.as_ptr(), bounds, 4);
            OK
        }
        None => FAILED,
    })
}

unsafe extern "C" fn fx_abort(_effect: *mut EffectObj) -> c_int {
    0
}

extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(p: *mut c_void);
}

struct MemObj {
    ptr: *mut c_void,
}

unsafe extern "C" fn fx_image_memory_alloc(_effect: *mut EffectObj, bytes: usize, out: *mut *mut MemObj) -> Status {
    let ptr = malloc(bytes.max(1));
    if ptr.is_null() {
        return ERR_MEMORY;
    }
    *out = Box::into_raw(Box::new(MemObj { ptr }));
    OK
}
unsafe extern "C" fn fx_image_memory_free(m: *mut MemObj) -> Status {
    if m.is_null() {
        return ERR_BAD_HANDLE;
    }
    let m = Box::from_raw(m);
    free(m.ptr);
    OK
}
unsafe extern "C" fn fx_image_memory_lock(m: *mut MemObj, out: *mut *mut c_void) -> Status {
    let Some(m) = m.as_ref() else { return ERR_BAD_HANDLE };
    *out = m.ptr;
    OK
}
unsafe extern "C" fn fx_image_memory_unlock(m: *mut MemObj) -> Status {
    if m.is_null() {
        ERR_BAD_HANDLE
    } else {
        OK
    }
}

#[repr(C)]
struct ImageEffectSuite {
    get_property_set: unsafe extern "C" fn(*mut EffectObj, *mut *mut PropSet) -> Status,
    get_param_set: unsafe extern "C" fn(*mut EffectObj, *mut *mut EffectObj) -> Status,
    clip_define: unsafe extern "C" fn(*mut EffectObj, *const c_char, *mut *mut PropSet) -> Status,
    clip_get_handle: unsafe extern "C" fn(*mut EffectObj, *const c_char, *mut *mut ClipObj, *mut *mut PropSet) -> Status,
    clip_get_property_set: unsafe extern "C" fn(*mut ClipObj, *mut *mut PropSet) -> Status,
    clip_get_image: unsafe extern "C" fn(*mut ClipObj, c_double, *const c_double, *mut *mut PropSet) -> Status,
    clip_release_image: unsafe extern "C" fn(*mut PropSet) -> Status,
    clip_get_rod: unsafe extern "C" fn(*mut ClipObj, c_double, *mut c_double) -> Status,
    abort: unsafe extern "C" fn(*mut EffectObj) -> c_int,
    image_memory_alloc: unsafe extern "C" fn(*mut EffectObj, usize, *mut *mut MemObj) -> Status,
    image_memory_free: unsafe extern "C" fn(*mut MemObj) -> Status,
    image_memory_lock: unsafe extern "C" fn(*mut MemObj, *mut *mut c_void) -> Status,
    image_memory_unlock: unsafe extern "C" fn(*mut MemObj) -> Status,
}

static IMAGE_EFFECT_SUITE: ImageEffectSuite = ImageEffectSuite {
    get_property_set: fx_get_property_set,
    get_param_set: fx_get_param_set,
    clip_define: fx_clip_define,
    clip_get_handle: fx_clip_get_handle,
    clip_get_property_set: fx_clip_get_property_set,
    clip_get_image: fx_clip_get_image,
    clip_release_image: fx_clip_release_image,
    clip_get_rod: fx_clip_get_rod,
    abort: fx_abort,
    image_memory_alloc: fx_image_memory_alloc,
    image_memory_free: fx_image_memory_free,
    image_memory_lock: fx_image_memory_lock,
    image_memory_unlock: fx_image_memory_unlock,
};

// ---------- パラメータスイート ----------

unsafe extern "C" fn param_define(set: *mut EffectObj, ty: *const c_char, name: *const c_char, out: *mut *mut PropSet) -> Status {
    let (Some(e), Some(ty), Some(name)) = (set.as_mut(), name_of(ty), name_of(name)) else { return ERR_BAD_HANDLE };
    if e.params.iter().any(|p| p.name == name) {
        return 6; // kOfxStatErrExists
    }
    if !ty.starts_with("OfxParamType") {
        return ERR_UNSUPPORTED;
    }
    e.params.push(new_param(&ty, &name));
    if !out.is_null() {
        *out = &mut e.params.last_mut().expect("param was just added").props;
    }
    OK
}

unsafe extern "C" fn param_get_handle(
    set: *mut EffectObj,
    name: *const c_char,
    param: *mut *mut ParamObj,
    props: *mut *mut PropSet,
) -> Status {
    let (Some(e), Some(name)) = (set.as_mut(), name_of(name)) else { return ERR_BAD_HANDLE };
    let Some(p) = e.params.iter_mut().find(|p| p.name == name) else { return ERR_UNKNOWN };
    if !param.is_null() {
        *param = &mut **p;
    }
    if !props.is_null() {
        *props = &mut p.props;
    }
    OK
}

unsafe extern "C" fn param_set_get_property_set(set: *mut EffectObj, out: *mut *mut PropSet) -> Status {
    let Some(e) = set.as_mut() else { return ERR_BAD_HANDLE };
    *out = &mut e.param_props;
    OK
}

unsafe extern "C" fn param_get_property_set(param: *mut ParamObj, out: *mut *mut PropSet) -> Status {
    let Some(p) = param.as_mut().filter(|p| p.magic == PARAM_MAGIC) else { return ERR_BAD_HANDLE };
    *out = &mut p.props;
    OK
}

unsafe extern "C" fn param_get_num_keys(param: *mut ParamObj, out: *mut c_uint) -> Status {
    if param.is_null() {
        return ERR_BAD_HANDLE;
    }
    // キーフレームはホスト側で評価して値として渡すので、プラグインからは常に 0 個に見える
    *out = 0;
    OK
}
unsafe extern "C" fn param_get_key_time(_p: *mut ParamObj, _nth: c_uint, _out: *mut c_double) -> Status {
    ERR_BAD_INDEX
}
unsafe extern "C" fn param_get_key_index(_p: *mut ParamObj, _time: c_double, _dir: c_int, _out: *mut c_int) -> Status {
    FAILED
}
unsafe extern "C" fn param_delete_key(_p: *mut ParamObj, _time: c_double) -> Status {
    ERR_BAD_INDEX
}
unsafe extern "C" fn param_delete_all_keys(_p: *mut ParamObj) -> Status {
    OK
}
unsafe extern "C" fn param_copy(to: *mut ParamObj, from: *mut ParamObj, _offset: c_double, _range: *const c_double) -> Status {
    let (Some(to), Some(from)) = (to.as_mut(), from.as_ref()) else { return ERR_BAD_HANDLE };
    to.value = from.value;
    to.text = from.text.clone();
    OK
}
unsafe extern "C" fn param_edit_begin(_set: *mut EffectObj, _name: *const c_char) -> Status {
    OK
}
unsafe extern "C" fn param_edit_end(_set: *mut EffectObj) -> Status {
    OK
}

// ---- C のシムから呼ばれる固定引数の関数 ----

#[no_mangle]
unsafe extern "C" fn znd_ofx_param_kind(param: *mut ParamObj) -> c_int {
    match param.as_ref() {
        Some(p) if p.magic == PARAM_MAGIC => p.kind,
        _ => -1,
    }
}

#[no_mangle]
unsafe extern "C" fn znd_ofx_param_get(
    param: *mut ParamObj,
    has_time: c_int,
    time: c_double,
    out: *mut c_double,
    text: *mut *const c_char,
) -> Status {
    let Some(p) = param.as_mut().filter(|p| p.magic == PARAM_MAGIC) else { return ERR_BAD_HANDLE };
    let mut value = p.value;
    let mut string: *const c_char = p.text.as_ptr();
    if has_time != 0 {
        let other = RENDER.with(|r| {
            let r = r.borrow();
            let cx = r.as_ref()?;
            if (time - cx.time).abs() < 1e-6 {
                return None;
            }
            (*cx.eval)(p, time)
        });
        if let Some((v, s)) = other {
            value = v;
            if let Some(s) = s {
                p.scratch = s;
                string = p.scratch.as_ptr();
            }
        }
    }
    std::ptr::copy_nonoverlapping(value.as_ptr(), out, 4);
    *text = string;
    OK
}

/// 時刻 `time`（フレーム）でのパラメータ値。描画中でなければ現在値。
unsafe fn value_at(p: &ParamObj, time: f64) -> [f64; 4] {
    RENDER
        .with(|r| {
            let r = r.borrow();
            let cx = r.as_ref()?;
            if (time - cx.time).abs() < 1e-9 {
                return None;
            }
            (*cx.eval)(p, time).map(|(v, _)| v)
        })
        .unwrap_or(p.value)
}

/// 微分（1 フレームあたりの変化量）。中心差分で求める。
#[no_mangle]
unsafe extern "C" fn znd_ofx_param_derive(param: *mut ParamObj, time: c_double, out: *mut c_double) -> Status {
    let Some(p) = param.as_ref().filter(|p| p.magic == PARAM_MAGIC) else { return ERR_BAD_HANDLE };
    const H: f64 = 0.01;
    let (a, b) = (value_at(p, time - H), value_at(p, time + H));
    for i in 0..4 {
        *out.add(i) = (b[i] - a[i]) / (2.0 * H);
    }
    OK
}

/// 積分（フレーム単位の時間について）。1 フレームを 4 分割した台形則で求める。
#[no_mangle]
unsafe extern "C" fn znd_ofx_param_integrate(param: *mut ParamObj, t1: c_double, t2: c_double, out: *mut c_double) -> Status {
    let Some(p) = param.as_ref().filter(|p| p.magic == PARAM_MAGIC) else { return ERR_BAD_HANDLE };
    if !t1.is_finite() || !t2.is_finite() {
        return ERR_VALUE;
    }
    let steps = (((t2 - t1).abs() * 4.0).ceil() as usize).clamp(1, 100_000);
    let h = (t2 - t1) / steps as f64;
    let mut sum = [0.0; 4];
    let mut prev = value_at(p, t1);
    for k in 1..=steps {
        let cur = value_at(p, t1 + h * k as f64);
        for i in 0..4 {
            sum[i] += (prev[i] + cur[i]) * 0.5 * h;
        }
        prev = cur;
    }
    std::ptr::copy_nonoverlapping(sum.as_ptr(), out, 4);
    OK
}

#[no_mangle]
unsafe extern "C" fn znd_ofx_param_set(param: *mut ParamObj, values: *const c_double, text: *const c_char) -> Status {
    let Some(p) = param.as_mut().filter(|p| p.magic == PARAM_MAGIC) else { return ERR_BAD_HANDLE };
    std::ptr::copy_nonoverlapping(values, p.value.as_mut_ptr(), 4);
    if !text.is_null() {
        p.text = CStr::from_ptr(text).to_owned();
    }
    OK
}

/// プラグインからのメッセージ（直近のものを保持して、一覧画面で見られるようにする）。
fn messages() -> &'static Mutex<Vec<String>> {
    static M: std::sync::OnceLock<Mutex<Vec<String>>> = std::sync::OnceLock::new();
    M.get_or_init(Mutex::default)
}

pub fn recent_messages() -> Vec<String> {
    messages().lock().unwrap().clone()
}

#[no_mangle]
unsafe extern "C" fn znd_ofx_message_text(_handle: *mut c_void, ty: *const c_char, _id: *const c_char, text: *const c_char) -> Status {
    let ty = name_of(ty).unwrap_or_default();
    let text = name_of(text).unwrap_or_default();
    let mut m = messages().lock().unwrap();
    if m.len() >= 50 {
        m.remove(0);
    }
    m.push(format!("{}: {text}", ty.trim_start_matches("OfxMessage")));
    // 質問形式のメッセージには「はい」と答える（対話できないため）
    if ty == "OfxMessageQuestion" {
        12
    } else {
        OK
    }
}

// ParamObj は C 側では中身を見ないポインタとしてだけ扱う
#[allow(improper_ctypes)]
extern "C" {
    fn znd_ofx_paramGetValue(param: *mut ParamObj, ...) -> Status;
    fn znd_ofx_paramGetValueAtTime(param: *mut ParamObj, time: c_double, ...) -> Status;
    fn znd_ofx_paramSetValue(param: *mut ParamObj, ...) -> Status;
    fn znd_ofx_paramSetValueAtTime(param: *mut ParamObj, time: c_double, ...) -> Status;
    fn znd_ofx_paramGetDerivative(param: *mut ParamObj, time: c_double, ...) -> Status;
    fn znd_ofx_paramGetIntegral(param: *mut ParamObj, t1: c_double, t2: c_double, ...) -> Status;
    fn znd_ofx_message(handle: *mut c_void, ty: *const c_char, id: *const c_char, format: *const c_char, ...) -> Status;
}

#[repr(C)]
struct ParameterSuite {
    define: unsafe extern "C" fn(*mut EffectObj, *const c_char, *const c_char, *mut *mut PropSet) -> Status,
    get_handle: unsafe extern "C" fn(*mut EffectObj, *const c_char, *mut *mut ParamObj, *mut *mut PropSet) -> Status,
    set_get_property_set: unsafe extern "C" fn(*mut EffectObj, *mut *mut PropSet) -> Status,
    get_property_set: unsafe extern "C" fn(*mut ParamObj, *mut *mut PropSet) -> Status,
    get_value: unsafe extern "C" fn(*mut ParamObj, ...) -> Status,
    get_value_at_time: unsafe extern "C" fn(*mut ParamObj, c_double, ...) -> Status,
    get_derivative: unsafe extern "C" fn(*mut ParamObj, c_double, ...) -> Status,
    get_integral: unsafe extern "C" fn(*mut ParamObj, c_double, c_double, ...) -> Status,
    set_value: unsafe extern "C" fn(*mut ParamObj, ...) -> Status,
    set_value_at_time: unsafe extern "C" fn(*mut ParamObj, c_double, ...) -> Status,
    get_num_keys: unsafe extern "C" fn(*mut ParamObj, *mut c_uint) -> Status,
    get_key_time: unsafe extern "C" fn(*mut ParamObj, c_uint, *mut c_double) -> Status,
    get_key_index: unsafe extern "C" fn(*mut ParamObj, c_double, c_int, *mut c_int) -> Status,
    delete_key: unsafe extern "C" fn(*mut ParamObj, c_double) -> Status,
    delete_all_keys: unsafe extern "C" fn(*mut ParamObj) -> Status,
    copy: unsafe extern "C" fn(*mut ParamObj, *mut ParamObj, c_double, *const c_double) -> Status,
    edit_begin: unsafe extern "C" fn(*mut EffectObj, *const c_char) -> Status,
    edit_end: unsafe extern "C" fn(*mut EffectObj) -> Status,
}

static PARAMETER_SUITE: ParameterSuite = ParameterSuite {
    define: param_define,
    get_handle: param_get_handle,
    set_get_property_set: param_set_get_property_set,
    get_property_set: param_get_property_set,
    get_value: znd_ofx_paramGetValue,
    get_value_at_time: znd_ofx_paramGetValueAtTime,
    get_derivative: znd_ofx_paramGetDerivative,
    get_integral: znd_ofx_paramGetIntegral,
    set_value: znd_ofx_paramSetValue,
    set_value_at_time: znd_ofx_paramSetValueAtTime,
    get_num_keys: param_get_num_keys,
    get_key_time: param_get_key_time,
    get_key_index: param_get_key_index,
    delete_key: param_delete_key,
    delete_all_keys: param_delete_all_keys,
    copy: param_copy,
    edit_begin: param_edit_begin,
    edit_end: param_edit_end,
};

// ---------- メモリ・マルチスレッド・メッセージ ----------

unsafe extern "C" fn memory_alloc(_handle: *mut c_void, bytes: usize, out: *mut *mut c_void) -> Status {
    let p = malloc(bytes.max(1));
    if p.is_null() {
        return ERR_MEMORY;
    }
    *out = p;
    OK
}
unsafe extern "C" fn memory_free(p: *mut c_void) -> Status {
    free(p);
    OK
}

#[repr(C)]
struct MemorySuite {
    alloc: unsafe extern "C" fn(*mut c_void, usize, *mut *mut c_void) -> Status,
    free: unsafe extern "C" fn(*mut c_void) -> Status,
}
static MEMORY_SUITE: MemorySuite = MemorySuite { alloc: memory_alloc, free: memory_free };

type ThreadFn = unsafe extern "C" fn(c_uint, c_uint, *mut c_void);

struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

unsafe extern "C" fn mt_multi_thread(func: Option<ThreadFn>, n: c_uint, arg: *mut c_void) -> Status {
    let Some(func) = func else { return FAILED };
    if SPAWNED.with(|s| s.get()) {
        return 6; // 入れ子では呼べない（kOfxStatErrExists）
    }
    let n = n.clamp(1, 64);
    let arg = SendPtr(arg);
    let arg = &arg;
    std::thread::scope(|s| {
        for i in 0..n {
            s.spawn(move || {
                THREAD_INDEX.with(|t| t.set(i));
                SPAWNED.with(|t| t.set(true));
                // SAFETY: プラグインが渡した関数を、プラグインが渡した引数で呼ぶ。
                unsafe { func(i, n, arg.0) };
            });
        }
    });
    OK
}
unsafe extern "C" fn mt_num_cpus(out: *mut c_uint) -> Status {
    *out = std::thread::available_parallelism().map(|n| n.get() as c_uint).unwrap_or(1);
    OK
}
unsafe extern "C" fn mt_index(out: *mut c_uint) -> Status {
    *out = THREAD_INDEX.with(|t| t.get());
    OK
}
unsafe extern "C" fn mt_is_spawned() -> c_int {
    SPAWNED.with(|t| t.get()) as c_int
}

struct OfxMutex {
    locked: Mutex<bool>,
    cv: Condvar,
}

unsafe extern "C" fn mt_mutex_create(out: *mut *mut OfxMutex, lock_count: c_int) -> Status {
    *out = Box::into_raw(Box::new(OfxMutex { locked: Mutex::new(lock_count > 0), cv: Condvar::new() }));
    OK
}
unsafe extern "C" fn mt_mutex_destroy(m: *mut OfxMutex) -> Status {
    if m.is_null() {
        return ERR_BAD_HANDLE;
    }
    drop(Box::from_raw(m));
    OK
}
unsafe extern "C" fn mt_mutex_lock(m: *mut OfxMutex) -> Status {
    let Some(m) = m.as_ref() else { return ERR_BAD_HANDLE };
    let mut g = m.locked.lock().unwrap_or_else(|e| e.into_inner());
    while *g {
        g = m.cv.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    *g = true;
    OK
}
unsafe extern "C" fn mt_mutex_unlock(m: *mut OfxMutex) -> Status {
    let Some(m) = m.as_ref() else { return ERR_BAD_HANDLE };
    *m.locked.lock().unwrap_or_else(|e| e.into_inner()) = false;
    m.cv.notify_one();
    OK
}
unsafe extern "C" fn mt_mutex_try_lock(m: *mut OfxMutex) -> Status {
    let Some(m) = m.as_ref() else { return ERR_BAD_HANDLE };
    let mut g = m.locked.lock().unwrap_or_else(|e| e.into_inner());
    if *g {
        FAILED
    } else {
        *g = true;
        OK
    }
}

#[repr(C)]
struct MultiThreadSuite {
    multi_thread: unsafe extern "C" fn(Option<ThreadFn>, c_uint, *mut c_void) -> Status,
    num_cpus: unsafe extern "C" fn(*mut c_uint) -> Status,
    index: unsafe extern "C" fn(*mut c_uint) -> Status,
    is_spawned: unsafe extern "C" fn() -> c_int,
    mutex_create: unsafe extern "C" fn(*mut *mut OfxMutex, c_int) -> Status,
    mutex_destroy: unsafe extern "C" fn(*mut OfxMutex) -> Status,
    mutex_lock: unsafe extern "C" fn(*mut OfxMutex) -> Status,
    mutex_unlock: unsafe extern "C" fn(*mut OfxMutex) -> Status,
    mutex_try_lock: unsafe extern "C" fn(*mut OfxMutex) -> Status,
}
static MULTI_THREAD_SUITE: MultiThreadSuite = MultiThreadSuite {
    multi_thread: mt_multi_thread,
    num_cpus: mt_num_cpus,
    index: mt_index,
    is_spawned: mt_is_spawned,
    mutex_create: mt_mutex_create,
    mutex_destroy: mt_mutex_destroy,
    mutex_lock: mt_mutex_lock,
    mutex_unlock: mt_mutex_unlock,
    mutex_try_lock: mt_mutex_try_lock,
};

unsafe extern "C" fn clear_persistent_message(_handle: *mut c_void) -> Status {
    OK
}

/// メッセージスイート V2（V1 は先頭の 1 関数だけを見る）。
#[repr(C)]
struct MessageSuite {
    message: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char, ...) -> Status,
    set_persistent_message: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char, ...) -> Status,
    clear_persistent_message: unsafe extern "C" fn(*mut c_void) -> Status,
}
static MESSAGE_SUITE: MessageSuite =
    MessageSuite { message: znd_ofx_message, set_persistent_message: znd_ofx_message, clear_persistent_message: clear_persistent_message };

// ---------- ホスト ----------

#[repr(C)]
struct OfxHost {
    host: *mut PropSet,
    fetch_suite: unsafe extern "C" fn(*mut PropSet, *const c_char, c_int) -> *const c_void,
}

unsafe extern "C" fn fetch_suite(_host: *mut PropSet, name: *const c_char, version: c_int) -> *const c_void {
    let Some(name) = name_of(name) else { return std::ptr::null() };
    match (name.as_str(), version) {
        ("OfxPropertySuite", 1) => &PROPERTY_SUITE as *const _ as *const c_void,
        ("OfxImageEffectSuite", 1) => &IMAGE_EFFECT_SUITE as *const _ as *const c_void,
        ("OfxParameterSuite", 1) => &PARAMETER_SUITE as *const _ as *const c_void,
        ("OfxMemorySuite", 1) => &MEMORY_SUITE as *const _ as *const c_void,
        ("OfxMultiThreadSuite", 1) => &MULTI_THREAD_SUITE as *const _ as *const c_void,
        ("OfxMessageSuite", 1 | 2) => &MESSAGE_SUITE as *const _ as *const c_void,
        _ => std::ptr::null(),
    }
}

fn host_props() -> PropSet {
    let mut p = PropSet::default();
    p.set_s("OfxPropType", &["OfxTypeImageEffectHost"])
        .set_s("OfxPropName", &["org.znd.host"])
        .set_s("OfxPropLabel", &["znd"])
        .set_i("OfxPropVersion", &[0, 1, 0])
        .set_s("OfxPropVersionLabel", &[env!("CARGO_PKG_VERSION")])
        .set_i("OfxPropAPIVersion", &[1, 4])
        .set_i("OfxImageEffectHostPropIsBackground", &[0])
        .set_i("OfxImageEffectPropSupportsOverlays", &[0])
        .set_i("OfxImageEffectPropSupportsMultiResolution", &[1])
        .set_i("OfxImageEffectPropSupportsTiles", &[0])
        .set_i("OfxImageEffectPropTemporalClipAccess", &[1])
        .set_s("OfxImageEffectPropSupportedComponents", &["OfxImageComponentRGBA"])
        .set_s("OfxImageEffectPropSupportedContexts", &["OfxImageEffectContextFilter", "OfxImageEffectContextGeneral"])
        .set_s("OfxImageEffectPropSupportedPixelDepths", &["OfxBitDepthFloat", "OfxBitDepthShort", "OfxBitDepthByte"])
        .set_i("OfxImageEffectPropMultipleClipDepths", &[0])
        .set_i("OfxImageEffectPropSupportsMultipleClipPARs", &[0])
        .set_i("OfxImageEffectPropSetableFrameRate", &[0])
        .set_i("OfxImageEffectPropSetableFielding", &[0])
        .set_i("OfxImageEffectInstancePropSequentialRender", &[0])
        .set_i("OfxParamHostPropSupportsCustomInteract", &[0])
        .set_i("OfxParamHostPropSupportsStringAnimation", &[0])
        .set_i("OfxParamHostPropSupportsChoiceAnimation", &[0])
        .set_i("OfxParamHostPropSupportsBooleanAnimation", &[0])
        .set_i("OfxParamHostPropSupportsCustomAnimation", &[0])
        .set_i("OfxParamHostPropSupportsParametricAnimation", &[0])
        .set_i("OfxParamHostPropSupportsStrChoice", &[0])
        .set_i("OfxParamHostPropSupportsStrChoiceAnimation", &[0])
        .set_s("OfxImageEffectHostPropNativeOrigin", &["kOfxImageEffectHostPropNativeOriginBottomLeft"])
        .set_s("OfxImageEffectPropOpenGLRenderSupported", &["false"])
        .set_s("OfxImageEffectPropOpenCLRenderSupported", &["false"])
        .set_s("OfxImageEffectPropMetalRenderSupported", &["false"])
        .set_s("OfxImageEffectPropCudaRenderSupported", &["false"])
        .set_s("OfxImageEffectPropCudaStreamSupported", &["false"])
        .set_i("OfxImageEffectPropRenderQualityDraft", &[0])
        .set_i("OfxParamHostPropMaxParameters", &[-1])
        .set_i("OfxParamHostPropMaxPages", &[0])
        .set_i("OfxParamHostPropPageRowColumnCount", &[0, 0])
        .set_p("OfxPropHostOSHandle", std::ptr::null_mut());
    p
}

#[repr(C)]
struct OfxPlugin {
    plugin_api: *const c_char,
    api_version: c_int,
    identifier: *const c_char,
    version_major: c_uint,
    version_minor: c_uint,
    set_host: Option<unsafe extern "C" fn(*mut OfxHost)>,
    main_entry: Option<unsafe extern "C" fn(*const c_char, *const c_void, *mut PropSet, *mut PropSet) -> Status>,
}

/// 値を持つパラメータと、znd 側のパラメータ名の対応。
#[derive(Clone, Debug)]
enum Mapping {
    Scalar,
    Bool,
    Choice,
    Color {
        alpha: bool,
    },
    /// 位置。`normalised` なら 0..1（左下原点）で渡す。
    Point {
        normalised: bool,
    },
    /// 複数成分を `名前.x` のように別々のスカラーとして持つ。
    Split(usize),
    Text,
}

struct Binary {
    _lib: libloading::Library,
    /// プラグインがポインタを保持するので、アドレスを固定して保持し続ける。
    _host: Box<OfxHost>,
    _host_props: Box<PropSet>,
}

struct State {
    instance: Box<EffectObj>,
    created: bool,
}

struct OfxEffect {
    info: PluginInfo,
    _binary: Arc<Binary>,
    plugin: *const OfxPlugin,
    context: String,
    /// (OFX パラメータ名, 対応)
    mapping: Vec<(String, Mapping)>,
    temporal: bool,
    depth: Depth,
    state: Mutex<State>,
}

// プラグインのポインタは Mutex（state）で直列化して使う。
unsafe impl Send for OfxEffect {}
unsafe impl Sync for OfxEffect {}

const SUFFIX: [&str; 3] = ["x", "y", "z"];

fn call(plugin: *const OfxPlugin, action: &str, handle: *const c_void, in_args: *mut PropSet, out_args: *mut PropSet) -> Status {
    if trace() {
        eprintln!("OpenFX: → {action}");
    }
    let action = cs(action);
    // SAFETY: plugin はライブラリが返した構造体で、ライブラリは Binary が保持している。
    unsafe {
        match (*plugin).main_entry {
            Some(f) => f(action.as_ptr(), handle, in_args, out_args),
            None => FAILED,
        }
    }
}

fn ok(status: Status) -> bool {
    status == OK || status == REPLY_DEFAULT
}

/// `.ofx.bundle`（または `.ofx` 本体）の中の、この OS・CPU 用のバイナリ。
fn binary_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = name.strip_suffix(".bundle").unwrap_or(&name);
    let arch_dirs: &[&str] = if cfg!(target_os = "macos") {
        &["MacOS", "MacOS-x86-64"]
    } else if cfg!(target_os = "windows") {
        &["Win64"]
    } else if cfg!(target_arch = "aarch64") {
        &["Linux-aarch64", "Linux-arm64"]
    } else {
        &["Linux-x86-64"]
    };
    arch_dirs
        .iter()
        .map(|d| path.join("Contents").join(d).join(stem))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("この環境用のバイナリがありません（Contents/{}/{stem}）", arch_dirs[0]))
}

pub(crate) fn load(path: &Path) -> Result<Vec<Arc<dyn Effect>>, String> {
    let bin = binary_path(path)?;
    // SAFETY: 動的ライブラリの読み込みは初期化コードを実行する。プラグインフォルダに置かれたファイルに限る。
    unsafe {
        let lib = libloading::Library::new(&bin).map_err(|e| format!("読み込めません: {e}"))?;
        let count: libloading::Symbol<unsafe extern "C" fn() -> c_int> = lib
            .get(b"OfxGetNumberOfPlugins")
            .map_err(|_| "OfxGetNumberOfPlugins がありません（OpenFX プラグインではありません）".to_string())?;
        let get: libloading::Symbol<unsafe extern "C" fn(c_int) -> *const OfxPlugin> =
            lib.get(b"OfxGetPlugin").map_err(|_| "OfxGetPlugin がありません".to_string())?;
        let n = count();
        let plugins: Vec<*const OfxPlugin> = (0..n.clamp(0, 4096)).map(|i| get(i)).filter(|p| !p.is_null()).collect();
        let mut host_props = Box::new(host_props());
        let mut host = Box::new(OfxHost { host: &mut *host_props, fetch_suite });
        let host_ptr: *mut OfxHost = &mut *host;
        let binary = Arc::new(Binary { _lib: lib, _host: host, _host_props: host_props });

        let mut out: Vec<Arc<dyn Effect>> = Vec::new();
        let mut errors = Vec::new();
        for plugin in plugins {
            let api = name_of((*plugin).plugin_api).unwrap_or_default();
            let id = name_of((*plugin).identifier).unwrap_or_default();
            if api != "OfxImageEffectPluginAPI" {
                continue;
            }
            match describe(plugin, &id, path, host_ptr, &binary) {
                Ok(e) => out.push(Arc::new(e)),
                Err(e) => errors.push(format!("{id}: {e}")),
            }
        }
        if out.is_empty() {
            return Err(if errors.is_empty() { "画像エフェクトが含まれていません".into() } else { errors.join(" / ") });
        }
        for e in errors {
            eprintln!("OpenFX: {}: {e}", path.display());
        }
        Ok(out)
    }
}

unsafe fn describe(plugin: *const OfxPlugin, id: &str, path: &Path, host: *mut OfxHost, binary: &Arc<Binary>) -> Result<OfxEffect, String> {
    let set_host = (*plugin).set_host.ok_or("setHost がありません")?;
    set_host(host);
    if !ok(call(plugin, "OfxActionLoad", std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut())) {
        return Err("読み込み（OfxActionLoad）に失敗しました".into());
    }
    let mut desc = new_effect("OfxTypeImageEffect", path);
    if !ok(call(plugin, "OfxActionDescribe", &*desc as *const EffectObj as *const c_void, std::ptr::null_mut(), std::ptr::null_mut())) {
        return Err("定義の取得（OfxActionDescribe）に失敗しました".into());
    }
    // float を優先し、無ければ 16bit、8bit の順（整数のプラグインでは 1 を超える値と階調の一部が失われる）
    let depths = desc.props.strings("OfxImageEffectPropSupportedPixelDepths");
    let depth = [Depth::Float, Depth::Short, Depth::Byte]
        .into_iter()
        .find(|d| depths.iter().any(|x| x == d.name()))
        .ok_or("対応する画素形式（float / 16bit / 8bit）がないプラグインです")?;
    let contexts = desc.props.strings("OfxImageEffectPropSupportedContexts");
    let context = ["OfxImageEffectContextFilter", "OfxImageEffectContextGeneral"]
        .into_iter()
        .find(|c| contexts.iter().any(|x| x == c))
        .ok_or("フィルター（入力 1 本）として使えないプラグインです（ジェネレーター・トランジション等は未対応）")?;
    let mut ctx_args = PropSet::default();
    ctx_args.set_s("OfxImageEffectPropContext", &[context]);
    if !ok(call(
        plugin,
        "OfxImageEffectActionDescribeInContext",
        &*desc as *const EffectObj as *const c_void,
        &mut ctx_args,
        std::ptr::null_mut(),
    )) {
        return Err("定義の取得（DescribeInContext）に失敗しました".into());
    }
    if !desc.clips.iter().any(|c| c.name == "Output") || !desc.clips.iter().any(|c| c.name == "Source") {
        return Err("Source / Output クリップがありません".into());
    }
    // 必須の入力が Source 以外にもあるものは使えない
    if let Some(extra) =
        desc.clips.iter().find(|c| c.name != "Output" && c.name != "Source" && c.props.int("OfxImageClipPropOptional") == 0)
    {
        return Err(format!("入力「{}」が必要なプラグインです（複数入力は未対応）", extra.name));
    }
    let temporal = desc.props.int("OfxImageEffectPropTemporalClipAccess") != 0
        || desc.clips.iter().any(|c| c.props.int("OfxImageEffectPropTemporalClipAccess") != 0);

    // パラメータを znd の定義に写す
    let mut mapping = Vec::new();
    let mut defs: Vec<(String, String, DynPd)> = Vec::new();
    for p in &mut desc.params {
        // 既定値を現在値にしておく
        match p.kind {
            K_STRING => p.text = cs(&p.props.string("OfxParamPropDefault")),
            K_NONE => {}
            _ => {
                for (i, v) in p.props.doubles("OfxParamPropDefault").into_iter().take(4).enumerate() {
                    p.value[i] = v;
                }
            }
        }
        if p.kind == K_NONE || p.props.int("OfxParamPropSecret") != 0 || p.ofx_type == "OfxParamTypeCustom" {
            continue;
        }
        let label = Some(p.props.string("OfxPropLabel")).filter(|l| !l.is_empty()).unwrap_or_else(|| p.name.clone());
        let d = p.value;
        let range = |i: usize| {
            let pick = |a: &str, b: &str| {
                let v = p.props.doubles(a);
                let v = if v.get(i).is_some_and(|x| x.abs() < 1e30) { v } else { p.props.doubles(b) };
                v.get(i).copied().filter(|x| x.abs() < 1e30)
            };
            (
                pick("OfxParamPropDisplayMin", "OfxParamPropMin").unwrap_or(-1e6),
                pick("OfxParamPropDisplayMax", "OfxParamPropMax").unwrap_or(1e6),
            )
        };
        let double_type = p.props.string("OfxParamPropDoubleType");
        let (map, defs_for): (Mapping, Vec<(String, String, DynPd)>) = match (p.ofx_type.as_str(), p.kind) {
            ("OfxParamTypeBoolean", _) => (Mapping::Bool, vec![(p.name.clone(), label, DynPd::B(d[0] != 0.0))]),
            ("OfxParamTypeChoice", _) => {
                let options = p.props.strings("OfxParamPropChoiceOption");
                if options.is_empty() {
                    continue;
                }
                let def = (d[0].max(0.0) as u32).min(options.len() as u32 - 1);
                (Mapping::Choice, vec![(p.name.clone(), label, DynPd::I(def, options))])
            }
            (_, K_DOUBLE | K_INT) => {
                let (lo, hi) = range(0);
                (Mapping::Scalar, vec![(p.name.clone(), label, DynPd::F(d[0], lo, hi))])
            }
            (_, K_RGBA) => (Mapping::Color { alpha: true }, vec![(p.name.clone(), label, DynPd::C(d))]),
            (_, K_RGB) => (Mapping::Color { alpha: false }, vec![(p.name.clone(), label, DynPd::C([d[0], d[1], d[2], 1.0]))]),
            (_, K_DOUBLE2D) if double_type.contains("XY") => {
                let normalised = double_type.contains("Normalised")
                    || p.props.string("OfxParamPropDefaultCoordinateSystem") == "OfxParamCoordinatesNormalised";
                // 正準座標（px）の既定値は、レイヤーの大きさが分からないので中央にする
                let def = if normalised { DynPd::P(d[0], 1.0 - d[1]) } else { DynPd::P(0.5, 0.5) };
                (Mapping::Point { normalised }, vec![(p.name.clone(), label, def)])
            }
            (_, K_DOUBLE2D | K_INT2D | K_DOUBLE3D | K_INT3D) => {
                let n = components(p.kind);
                let v = (0..n)
                    .map(|i| {
                        let (lo, hi) = range(i);
                        (format!("{}.{}", p.name, SUFFIX[i]), format!("{label} {}", SUFFIX[i].to_uppercase()), DynPd::F(d[i], lo, hi))
                    })
                    .collect();
                (Mapping::Split(n), v)
            }
            (_, K_STRING) => (Mapping::Text, vec![(p.name.clone(), label, DynPd::S)]),
            _ => continue,
        };
        mapping.push((p.name.clone(), map));
        defs.extend(defs_for);
    }

    let label = Some(desc.props.string("OfxPropLabel")).filter(|l| !l.is_empty()).unwrap_or_else(|| id.to_string());
    let grouping = desc.props.string("OfxImageEffectPluginPropGrouping");
    let category = if grouping.is_empty() { "OpenFX".to_string() } else { format!("OpenFX / {grouping}") };
    let kind = format!("ofx:{id}");
    register_effect(DynEffect {
        kind: kind.clone(),
        label: label.clone(),
        category: category.clone(),
        time_dependent: temporal,
        params: defs.clone(),
    });

    // インスタンスは定義の写しから作る（最初の描画のときに CreateInstance を呼ぶ）
    let mut instance = new_effect("OfxTypeImageEffectInstance", path);
    for (k, v) in &desc.props.map {
        if k != "OfxPropType" {
            instance.props.map.insert(k.clone(), v.clone());
        }
    }
    instance.props.set_s("OfxImageEffectPropContext", &[context]);
    for p in &desc.params {
        let mut q = new_param(&p.ofx_type, &p.name);
        q.props.map = p.props.map.clone();
        q.props.set_s("OfxPropType", &["OfxTypeParameterInstance"]);
        q.value = p.value;
        q.text = p.text.clone();
        instance.params.push(q);
    }
    for c in &desc.clips {
        let mut q = new_clip(&c.name);
        q.props.map = c.props.map.clone();
        q.props
            .set_s("OfxImageEffectPropPixelDepth", &[depth.name()])
            .set_s("OfxImageEffectPropComponents", &["OfxImageComponentRGBA"])
            .set_s("OfxImageClipPropUnmappedPixelDepth", &[depth.name()])
            .set_s("OfxImageClipPropUnmappedComponents", &["OfxImageComponentRGBA"])
            .set_s("OfxImageEffectPropPreMultiplication", &["OfxImageAlphaPremultiplied"])
            .set_d("OfxImagePropPixelAspectRatio", &[1.0])
            .set_s("OfxImageClipPropFieldOrder", &["OfxFieldNone"])
            .set_i("OfxImageClipPropConnected", &[(c.name == "Source" || c.name == "Output") as c_int])
            .set_i("OfxImageClipPropContinuousSamples", &[0]);
        instance.clips.push(q);
    }
    Ok(OfxEffect {
        info: PluginInfo {
            kind,
            label,
            category,
            vendor: id.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_default(),
            version: format!("{}.{}", (*plugin).version_major, (*plugin).version_minor),
            license: String::new(),
            path: path.to_path_buf(),
            api: PluginApi::OpenFx,
            param_count: defs.len(),
        },
        _binary: binary.clone(),
        plugin,
        context: context.to_string(),
        mapping,
        temporal,
        depth,
        state: Mutex::new(State { instance, created: false }),
    })
}

impl OfxEffect {
    /// znd のパラメータ値（時刻評価済み）から、OFX パラメータ 1 つの値を作る。
    /// `canon_h` はレイヤーの高さ（正準座標）。
    fn value_of(
        map: &Mapping,
        name: &str,
        p: &EvalParams,
        cx: &crate::color::ColorXf,
        canon_w: f64,
        canon_h: f64,
    ) -> ([f64; 4], Option<CString>) {
        match map {
            Mapping::Scalar => ([p.f(name), 0.0, 0.0, 0.0], None),
            Mapping::Bool => ([p.b(name) as u8 as f64, 0.0, 0.0, 0.0], None),
            Mapping::Choice => ([p.i(name) as f64, 0.0, 0.0, 0.0], None),
            Mapping::Color { alpha } => {
                let c = p.c(name);
                // 画像と同じ空間（作業色空間のリニア値）で渡す
                let l = cx.rgb([c[0] as f32, c[1] as f32, c[2] as f32]);
                ([l[0] as f64, l[1] as f64, l[2] as f64, if *alpha { c[3] } else { 1.0 }], None)
            }
            Mapping::Point { normalised } => {
                let pt = p.p(name);
                // OFX は左下原点
                if *normalised {
                    ([pt[0] / canon_w.max(1e-9), 1.0 - pt[1] / canon_h.max(1e-9), 0.0, 0.0], None)
                } else {
                    ([pt[0], canon_h - pt[1], 0.0, 0.0], None)
                }
            }
            Mapping::Split(n) => {
                let mut v = [0.0; 4];
                for (i, slot) in v.iter_mut().enumerate().take(*n) {
                    *slot = p.f(&format!("{name}.{}", SUFFIX[i]));
                }
                (v, None)
            }
            Mapping::Text => {
                let s = match p.0.get(name) {
                    Some(EvalValue::S(s)) => s.as_str(),
                    _ => "",
                };
                ([0.0; 4], Some(cs(s)))
            }
        }
    }
}

impl Effect for OfxEffect {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn apply(&self, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
        let (w, h) = (buf.width, buf.height);
        if w == 0 || h == 0 {
            return buf;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let fps = ctx.frame_rate.fps();
        let time = ctx.time.seconds() * fps;
        let scale = (ctx.scale as f64).max(1e-6);
        let (canon_w, canon_h) = (w as f64 / scale, h as f64 / scale);
        let inst_ptr: *mut EffectObj = &mut *state.instance;

        {
            let props = &mut state.instance.props;
            props
                .set_d("OfxImageEffectPropProjectSize", &[canon_w, canon_h])
                .set_d("OfxImageEffectPropProjectOffset", &[0.0, 0.0])
                .set_d("OfxImageEffectPropProjectExtent", &[canon_w, canon_h])
                .set_d("OfxImageEffectPropPixelAspectRatio", &[1.0])
                .set_d("OfxImageEffectInstancePropEffectDuration", &[1e6])
                .set_d("OfxImageEffectPropFrameRate", &[fps])
                .set_i("OfxPropIsInteractive", &[0])
                .set_i("OfxImageEffectInstancePropSequentialRender", &[0]);
            for c in &mut state.instance.clips {
                c.props
                    .set_d("OfxImageEffectPropFrameRate", &[fps])
                    .set_d("OfxImageEffectPropUnmappedFrameRate", &[fps])
                    .set_d("OfxImageEffectPropFrameRange", &[0.0, 1e6])
                    .set_d("OfxImageEffectPropUnmappedFrameRange", &[0.0, 1e6]);
            }
        }
        // 現在時刻のパラメータ値
        for (name, map) in &self.mapping {
            let (v, s) = Self::value_of(map, name, p, ctx.cx, canon_w, canon_h);
            if let Some(param) = state.instance.params.iter_mut().find(|q| &q.name == name) {
                param.value = v;
                if let Some(s) = s {
                    param.text = s;
                }
            }
        }
        if !state.created {
            if !ok(call(self.plugin, "OfxActionCreateInstance", inst_ptr as *const c_void, std::ptr::null_mut(), std::ptr::null_mut())) {
                eprintln!("OpenFX: {}: インスタンスを作れません", self.info.kind);
                return buf;
            }
            state.created = true;
        }

        // 別時刻の入力とパラメータ（プラグインが求めたときだけ評価する）
        let depth = self.depth;
        let fetch = |frame: f64| -> Option<Pixels> {
            let t = Time::from_seconds(frame / fps);
            let other = (ctx.fetch_self)(t)?;
            (other.width == w && other.height == h).then(|| Pixels::from_frame(&other.data, w, h, depth))
        };
        let mapping = &self.mapping;
        let eval = |param: &ParamObj, frame: f64| -> Option<([f64; 4], Option<CString>)> {
            let (name, map) = mapping.iter().find(|(n, _)| n == &param.name)?;
            let values = (ctx.eval_at)(Time::from_seconds(frame / fps));
            Some(Self::value_of(map, name, &values, ctx.cx, canon_w, canon_h))
        };
        let fetch_dyn: &dyn Fn(f64) -> Option<Pixels> = &fetch;
        let eval_dyn: &dyn Fn(&ParamObj, f64) -> Option<([f64; 4], Option<CString>)> = &eval;
        // SAFETY: これらの参照は、この関数の終わりで RENDER を空にするまでしか使われない。
        let cx = unsafe {
            RenderCx {
                src: Pixels::from_frame(&buf.data, w, h, depth),
                dst: Pixels::zeroed(w, h, depth),
                depth,
                w,
                h,
                time,
                fps,
                scale,
                fetch: std::mem::transmute::<&dyn Fn(f64) -> Option<Pixels>, *const (dyn Fn(f64) -> Option<Pixels> + 'static)>(fetch_dyn),
                eval: std::mem::transmute::<
                    &dyn Fn(&ParamObj, f64) -> Option<([f64; 4], Option<CString>)>,
                    *const (dyn Fn(&ParamObj, f64) -> Option<([f64; 4], Option<CString>)> + 'static),
                >(eval_dyn),
            }
        };
        RENDER.with(|r| *r.borrow_mut() = Some(cx));

        let window = [0, 0, w as c_int, h as c_int];
        let mut seq = PropSet::default();
        seq.set_d("OfxImageEffectPropFrameRange", &[time, time])
            .set_d("OfxImageEffectPropFrameStep", &[1.0])
            .set_i("OfxPropIsInteractive", &[0])
            .set_d("OfxImageEffectPropRenderScale", &[scale, scale])
            .set_i("OfxImageEffectPropSequentialRenderStatus", &[0])
            .set_i("OfxImageEffectPropInteractiveRenderStatus", &[0]);
        let mut args = PropSet::default();
        args.set_d("OfxPropTime", &[time])
            .set_s("OfxImageEffectPropFieldToRender", &["OfxFieldNone"])
            .set_i("OfxImageEffectPropRenderWindow", &window)
            .set_d("OfxImageEffectPropRenderScale", &[scale, scale])
            .set_i("OfxImageEffectPropSequentialRenderStatus", &[0])
            .set_i("OfxImageEffectPropInteractiveRenderStatus", &[0])
            .set_i("OfxImageEffectPropRenderQualityDraft", &[0]);
        let handle = inst_ptr as *const c_void;
        let begun = ok(call(self.plugin, "OfxImageEffectActionBeginSequenceRender", handle, &mut seq, std::ptr::null_mut()));
        let status = call(self.plugin, "OfxImageEffectActionRender", handle, &mut args, std::ptr::null_mut());
        if begun {
            call(self.plugin, "OfxImageEffectActionEndSequenceRender", handle, &mut seq, std::ptr::null_mut());
        }
        let cx = RENDER.with(|r| r.borrow_mut().take()).expect("render context was set above");
        let _ = (cx.fps, &self.context, self.temporal);
        if status == OK {
            let out = cx.dst.to_frame(w, h, depth);
            if out.iter().all(|v| v.is_finite()) {
                buf.data = out;
            }
        } else {
            eprintln!("OpenFX: {}: 描画に失敗しました（状態 {status}）", self.info.kind);
        }
        buf
    }
}

impl Drop for OfxEffect {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if state.created {
                let handle = &mut *state.instance as *mut EffectObj as *const c_void;
                call(self.plugin, "OfxActionDestroyInstance", handle, std::ptr::null_mut(), std::ptr::null_mut());
            }
        }
    }
}
