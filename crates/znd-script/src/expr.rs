//! エクスプレッションの評価。QuickJS をスレッドごとに 1 つ持ち、プロジェクトへのアクセスは
//! 少数のネイティブ関数（`__eval_prop` など）を通して行う。

use rquickjs::{Context, Ctx, Exception, Function, Runtime, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use znd_core::effect::{effect_def, ParamValue};
use znd_core::eval::Affine;
use znd_core::visit::{find_prop, has_expressions, visit_layer, visit_layer_mut};
use znd_core::{CompId, Composition, Layer, LayerId, LayerKind, Project, Time};
use znd_render::Resolver;

const PRELUDE: &str = include_str!("prelude.js");
/// `with` を使うので、この関数だけは非 strict で定義する。
/// 内側ほど優先: 時刻・値などの変数 → このプロパティのメソッド → このレイヤーのメンバー → グローバル関数。
const RUN: &str =
    "function __run(__src, __layer, __prop, __global) { with (__layer) { with (__prop) { with (__global) { return eval(__src); } } } }";

const MAX_NESTING: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct ExprError {
    pub layer: String,
    pub path: String,
    /// エクスプレッション内の行番号（1 始まり）。特定できないときは `None`。
    pub line: Option<u32>,
    pub message: String,
}

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "レイヤー「{}」の {}（{} 行目）: {}", self.layer, self.path, l, self.message),
            None => write!(f, "レイヤー「{}」の {}: {}", self.layer, self.path, self.message),
        }
    }
}

struct State {
    project: *const Project,
    /// 評価中のプロパティ（循環参照の検出用）。
    stack: Vec<(String, usize, String)>,
    /// 最も内側で起きたエラー。
    error: Option<ExprError>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static JS: (Runtime, Context) = init();
    /// このスレッドの JS に読み込み済みの、プラグインのエクスプレッション用ライブラリの世代。
    static LIB_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// プラグインが追加したエクスプレッション用ライブラリを、まだならこのスレッドの JS に読み込む。
/// ライブラリの中のエラーは、そのライブラリを飛ばして続ける（他のエクスプレッションを巻き込まない）。
fn sync_libraries(ctx: &Ctx) {
    let (generation, libs) = znd_render::plugins::hooks::expression_libraries();
    if LIB_GEN.with(|g| g.get()) == generation {
        return;
    }
    LIB_GEN.with(|g| g.set(generation));
    for (name, src) in libs.iter() {
        if ctx.eval::<(), _>(src.as_str()).is_err() {
            let caught = ctx.catch();
            let (message, _) = describe(&caught);
            eprintln!("エクスプレッション用ライブラリ {name}: {message}");
        }
    }
}

fn project<'a>() -> Option<&'a Project> {
    // SAFETY: ポインタは `with_project` の呼び出し中だけ設定され、その間プロジェクトは借用されたまま生きている。
    STATE.with(|s| s.borrow().as_ref().map(|st| unsafe { &*st.project }))
}

fn throw<'js, T>(ctx: &Ctx<'js>, message: &str) -> rquickjs::Result<T> {
    Err(Exception::throw_message(ctx, message))
}

fn find_comp<'a>(p: &'a Project, id_or_name: &str) -> Option<&'a Composition> {
    p.compositions.iter().find(|c| c.id.0 == id_or_name).or_else(|| p.compositions.iter().find(|c| c.name == id_or_name))
}

fn layer_at<'a, 'js>(ctx: &Ctx<'js>, comp: &str, index: i32) -> rquickjs::Result<(&'a Composition, &'a Layer)> {
    let Some(p) = project() else { return throw(ctx, "プロジェクトがありません") };
    let Some(c) = find_comp(p, comp) else { return throw(ctx, &format!("コンポ「{comp}」が見つかりません")) };
    match c.layers.get(index.max(0) as usize) {
        Some(l) if index >= 0 => Ok((c, l)),
        _ => throw(ctx, &format!("レイヤー {} は範囲外です", index + 1)),
    }
}

/// エクスプレッション評価後の値で組んだ 2D トランスフォーム（親チェーン込み）。
fn resolved_world(ctx: &Ctx, comp: &Composition, index: usize, t: f64, depth: usize) -> rquickjs::Result<Affine> {
    let layer = &comp.layers[index];
    let get = |path: &str| eval_prop(ctx, comp, index, path, t, false);
    let (a, p, s, r) = (get("transform.anchor")?, get("transform.position")?, get("transform.scale")?, get("transform.rotation")?);
    let local = Affine::translate(p[0], p[1])
        .then_after(&Affine::rotate(r[0]))
        .then_after(&Affine::scale(s[0] / 100.0, s[1] / 100.0))
        .then_after(&Affine::translate(-a[0], -a[1]));
    match layer.parent.as_ref().and_then(|id| comp.layer_index(id)) {
        Some(pi) if depth < comp.layers.len() => Ok(resolved_world(ctx, comp, pi, t, depth + 1)?.then_after(&local)),
        _ => Ok(local),
    }
}

/// プロパティを評価する。`raw` ならキーフレームだけの値、そうでなければエクスプレッションも適用する。
fn eval_prop(ctx: &Ctx, comp: &Composition, index: usize, path: &str, t: f64, raw: bool) -> rquickjs::Result<Vec<f64>> {
    let layer = &comp.layers[index];
    let Some(prop) = find_prop(layer, path) else {
        return throw(ctx, &format!("レイヤー「{}」にプロパティ {path} がありません", layer.name));
    };
    let Some(base) = prop.eval(Time::from_seconds(t)) else {
        return throw(ctx, &format!("{path} は数値のプロパティではありません"));
    };
    let Some(src) = prop.expression().filter(|_| !raw) else { return Ok(base) };
    let key = (comp.id.0.clone(), index, path.to_string());
    let (cyclic, depth) = STATE.with(|s| {
        let s = s.borrow();
        let st = s.as_ref().unwrap();
        (st.stack.contains(&key), st.stack.len())
    });
    if cyclic {
        // 自分自身を参照した場合は、エクスプレッション適用前の値を返す
        return Ok(base);
    }
    if depth >= MAX_NESTING {
        return throw(ctx, "エクスプレッションの参照が深すぎます（循環参照の可能性）");
    }
    STATE.with(|s| s.borrow_mut().as_mut().unwrap().stack.push(key));
    let f: Function = ctx.globals().get("__evalExpr")?;
    let result: rquickjs::Result<Value> = f.call((src, comp.id.0.as_str(), index as i32, path, t, base.clone()));
    STATE.with(|s| s.borrow_mut().as_mut().unwrap().stack.pop());
    let value = match result {
        Ok(v) => v,
        Err(e) => {
            // 最初（最も内側）のエラーを、行番号付きで覚えておく
            let caught = ctx.catch();
            let (message, line) = describe(&caught);
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                let st = s.as_mut().unwrap();
                if st.error.is_none() {
                    st.error = Some(ExprError { layer: layer.name.clone(), path: path.to_string(), line, message });
                }
            });
            // 外側の式にも「参照先でエラー」として伝える
            let _ = e;
            return throw(ctx, "参照先のエクスプレッションでエラーが起きました");
        }
    };
    let out = to_numbers(&value);
    match out {
        Some(v) if v.len() >= base.len() => Ok(v[..base.len()].to_vec()),
        Some(v) => fail(ctx, layer, path, &format!("結果の次元が足りません（{} 次元が必要、{} 次元が返されました）", base.len(), v.len())),
        None => fail(ctx, layer, path, "結果が数値または数値の配列ではありません"),
    }
}

fn fail<T>(ctx: &Ctx, layer: &Layer, path: &str, message: &str) -> rquickjs::Result<T> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let st = s.as_mut().unwrap();
        if st.error.is_none() {
            st.error = Some(ExprError { layer: layer.name.clone(), path: path.to_string(), line: None, message: message.to_string() });
        }
    });
    throw(ctx, message)
}

fn to_numbers(v: &Value) -> Option<Vec<f64>> {
    if let Some(n) = v.as_number() {
        return n.is_finite().then(|| vec![n]);
    }
    if let Some(b) = v.as_bool() {
        return Some(vec![b as u8 as f64]);
    }
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr.iter::<Value>() {
        let n = item.ok()?.as_number()?;
        if !n.is_finite() {
            return None;
        }
        out.push(n);
    }
    Some(out)
}

/// 例外から (メッセージ, エクスプレッション内の行番号) を取り出す。
fn describe(caught: &Value) -> (String, Option<u32>) {
    let Some(ex) = caught.as_object().and_then(|o| Exception::from_object(o.clone())) else {
        return (format!("{caught:?}"), None);
    };
    let name: String = ex.get("name").unwrap_or_default();
    let message = ex.message().unwrap_or_default();
    let stack = ex.stack().unwrap_or_default();
    // エクスプレッション本体のフレームは「<input>:行:桁」と表示される（ライブラリ側は eval_script）。
    // 最初に現れるものが、エクスプレッション内でエラーに最も近い行。
    let line = stack.lines().find_map(|l| {
        let rest = l.split("<input>:").nth(1)?;
        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok()
    });
    let text = if name.is_empty() || message.starts_with(&name) { message } else { format!("{name}: {message}") };
    (text, line)
}

fn init() -> (Runtime, Context) {
    let rt = Runtime::new().expect("failed to create JS runtime");
    rt.set_memory_limit(256 * 1024 * 1024);
    rt.set_max_stack_size(1024 * 1024);
    let context = Context::full(&rt).expect("failed to create JS context");
    context.with(|ctx| {
        let g = ctx.globals();
        macro_rules! reg {
            ($name:expr, $f:expr $(,)?) => {
                g.set($name, $f.expect("native")).expect("register native")
            };
        }

        reg!(
            "__eval_prop",
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'_>, comp: String, index: i32, path: String, t: f64, raw: bool| -> rquickjs::Result<Vec<f64>> {
                    let (c, _) = layer_at(&ctx, &comp, index)?;
                    eval_prop(&ctx, c, index as usize, &path, t, raw)
                }
            ),
        );
        reg!(
            "__keys",
            Function::new(ctx.clone(), |ctx: Ctx<'_>, comp: String, index: i32, path: String| -> rquickjs::Result<Vec<f64>> {
                let (_, l) = layer_at(&ctx, &comp, index)?;
                Ok(find_prop(l, &path).map(|p| p.key_times().iter().map(|t| t.seconds()).collect()).unwrap_or_default())
            }),
        );
        reg!(
            "__layer_info",
            Function::new(ctx.clone(), |ctx: Ctx<'_>, comp: String, which: String| -> rquickjs::Result<String> {
                let Some(p) = project() else { return throw(&ctx, "プロジェクトがありません") };
                let Some(c) = find_comp(p, &comp) else { return throw(&ctx, &format!("コンポ「{comp}」が見つかりません")) };
                // "#3" は番号、それ以外は名前
                let index = match which.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
                    Some(n) => (n >= 1 && n <= c.layers.len()).then(|| n - 1),
                    None => c.layers.iter().position(|l| l.name == which),
                };
                let Some(i) = index else { return Ok("{\"index\":-1}".into()) };
                let l = &c.layers[i];
                let (w, h) = p.source_size(&l.kind).unwrap_or((c.width as f64, c.height as f64));
                let parent = l.parent.as_ref().and_then(|id| c.layer_index(id)).map(|v| v as i64).unwrap_or(-1);
                Ok(serde_json::json!({
                    "index": i + 1, "name": l.name, "inPoint": l.in_point.seconds(), "outPoint": l.out_point.seconds(),
                    "startTime": l.start_time.seconds(), "width": w, "height": h, "parent": parent
                })
                .to_string())
            }),
        );
        reg!(
            "__comp_info",
            Function::new(ctx.clone(), |ctx: Ctx<'_>, which: String| -> rquickjs::Result<String> {
                let Some(p) = project() else { return throw(&ctx, "プロジェクトがありません") };
                let Some(c) = find_comp(p, &which) else { return throw(&ctx, &format!("コンポ「{which}」が見つかりません")) };
                Ok(serde_json::json!({
                    "id": c.id.0, "name": c.name, "width": c.width, "height": c.height, "duration": c.duration.seconds(),
                    "frameDuration": 1.0 / c.frame_rate.fps(), "numLayers": c.layers.len()
                })
                .to_string())
            }),
        );
        reg!(
            "__markers",
            Function::new(ctx.clone(), |ctx: Ctx<'_>, comp: String, index: i32| -> rquickjs::Result<String> {
                let Some(p) = project() else { return throw(&ctx, "プロジェクトがありません") };
                let Some(c) = find_comp(p, &comp) else { return throw(&ctx, &format!("コンポ「{comp}」が見つかりません")) };
                let markers = if index < 0 { &c.markers } else { &layer_at(&ctx, &comp, index)?.1.markers };
                let list: Vec<_> = markers
                    .iter()
                    .map(|m| serde_json::json!({"time": m.time.seconds(), "duration": m.duration.seconds(), "comment": m.comment}))
                    .collect();
                Ok(serde_json::Value::Array(list).to_string())
            }),
        );
        reg!(
            "__fx_path",
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'_>, comp: String, index: i32, effect: String, param: String| -> rquickjs::Result<String> {
                    let (c, l) = layer_at(&ctx, &comp, index)?;
                    // エフェクトは名前または 1 始まりの番号で指定する
                    let fi = match effect.parse::<usize>() {
                        Ok(n) if n >= 1 && n <= l.effects.len() => Some(n - 1),
                        _ => l.effects.iter().position(|e| e.name == effect),
                    };
                    let Some(fi) = fi else { return Ok(String::new()) };
                    let fx = &l.effects[fi];
                    // パラメータは内部名・表示名・1 始まりの番号のどれでもよい
                    let def = effect_def(&fx.kind);
                    let name = def.and_then(|d| {
                        d.params
                            .iter()
                            .find(|p| p.name == param || p.label == param)
                            .or_else(|| param.parse::<usize>().ok().and_then(|n| d.params.get(n.wrapping_sub(1))))
                            .map(|p| p.name)
                    });
                    let Some(name) = name else { return Ok(String::new()) };
                    match fx.params.get(name) {
                        Some(ParamValue::Layer(Some(id))) => Ok(c.layer_index(id).map(|i| format!("layer:{i}")).unwrap_or_default()),
                        Some(ParamValue::Scalar(_) | ParamValue::Point(_) | ParamValue::Color(_)) => Ok(format!("fx.{fi}.{name}")),
                        // チェックボックス・選択肢はキーフレームを持たないので、値そのものを返す（真は 1、選択肢は 0 始まりの番号）
                        Some(ParamValue::Bool(b)) => Ok(format!("const:{}", *b as u8)),
                        Some(ParamValue::Choice(i)) => Ok(format!("const:{i}")),
                        _ => Ok(String::new()),
                    }
                }
            ),
        );
        reg!(
            "__source_rect",
            Function::new(ctx.clone(), |ctx: Ctx<'_>, comp: String, index: i32, t: f64| -> rquickjs::Result<Vec<f64>> {
                let (c, l) = layer_at(&ctx, &comp, index)?;
                let Some(p) = project() else { return throw(&ctx, "プロジェクトがありません") };
                Ok(source_rect(p, c, l, Time::from_seconds(t)).to_vec())
            }),
        );
        reg!(
            "__to_comp",
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'_>, comp: String, index: i32, x: f64, y: f64, t: f64, inverse: bool| -> rquickjs::Result<Vec<f64>> {
                    let (c, _) = layer_at(&ctx, &comp, index)?;
                    let m = resolved_world(&ctx, c, index as usize, t, 0)?;
                    let m = if inverse {
                        match m.inverse() {
                            Some(i) => i,
                            None => return throw(&ctx, "トランスフォームが逆変換できません（スケール 0）"),
                        }
                    } else {
                        m
                    };
                    let (px, py) = m.apply(x, y);
                    Ok(vec![px, py])
                }
            ),
        );
        reg!(
            "__text",
            Function::new(ctx.clone(), |ctx: Ctx<'_>, comp: String, index: i32| -> rquickjs::Result<String> {
                let (_, l) = layer_at(&ctx, &comp, index)?;
                Ok(match &l.kind {
                    LayerKind::Text { document, .. } => document.text.clone(),
                    _ => String::new(),
                })
            }),
        );

        ctx.eval::<(), _>(PRELUDE).expect("prelude");
        let mut opts = rquickjs::context::EvalOptions::default();
        opts.strict = false;
        ctx.eval_with_options::<(), _>(RUN, opts).expect("run helper");
    });
    (rt, context)
}

/// レイヤーのソース矩形 [左, 上, 幅, 高さ]（レイヤー座標）。
pub fn source_rect(project: &Project, comp: &Composition, layer: &Layer, t: Time) -> [f64; 4] {
    match &layer.kind {
        LayerKind::Text { document, animators, .. } => {
            let (lay, _) = znd_render::text::animated_layout(document, animators, t, &|_, _, _, v| v);
            lay.bounds
        }
        LayerKind::Shape { items } => {
            let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for path in znd_render::shape::shape_paths(items, t) {
                for p in znd_render::path::flatten(&path, 0.25).pts {
                    b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                }
            }
            if b[0] > b[2] {
                [0.0; 4]
            } else {
                [b[0], b[1], b[2] - b[0], b[3] - b[1]]
            }
        }
        kind => {
            let (w, h) = project.source_size(kind).unwrap_or((comp.width as f64, comp.height as f64));
            [0.0, 0.0, w, h]
        }
    }
}

/// プロジェクトを設定した状態で `f` を実行する。エラーは最も内側のものを返す。
fn with_project<T>(project: &Project, f: impl FnOnce(&Ctx) -> rquickjs::Result<T>) -> Result<T, ExprError> {
    JS.with(|(_, context)| {
        // 入れ子で呼ばれた場合（エクスプレッションの評価中に別の評価が始まる）は状態を引き継ぐ
        let fresh = STATE.with(|s| s.borrow().is_none());
        if fresh {
            STATE.with(|s| *s.borrow_mut() = Some(State { project, stack: Vec::new(), error: None }));
        }
        let result = context.with(|ctx| {
            if fresh {
                sync_libraries(&ctx);
            }
            f(&ctx).map_err(|_| {
                let caught = ctx.catch();
                STATE.with(|s| s.borrow_mut().as_mut().and_then(|st| st.error.take())).unwrap_or_else(|| {
                    let (message, line) = describe(&caught);
                    ExprError { layer: String::new(), path: String::new(), line, message }
                })
            })
        });
        if fresh {
            STATE.with(|s| *s.borrow_mut() = None);
        }
        result
    })
}

type CacheKey = (usize, CompId, Time);

/// エクスプレッションエンジン。レンダラーに `Resolver` として渡す。
#[derive(Default)]
pub struct ExprEngine {
    /// (プロジェクトの番地, コンポ, 時刻) → 解決済みコンポ。プロジェクトを編集したら `clear` すること。
    cache: Mutex<HashMap<CacheKey, Result<Option<Arc<Composition>>, String>>>,
}

const CACHE_LIMIT: usize = 512;

impl ExprEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// プロパティ 1 つを評価する（エクスプレッションがあれば適用）。
    pub fn eval_property(&self, project: &Project, comp: &CompId, layer: &LayerId, path: &str, t: Time) -> Result<Vec<f64>, ExprError> {
        let err = |message: String| ExprError { layer: layer.0.clone(), path: path.to_string(), line: None, message };
        let c = project.comp(comp).ok_or_else(|| err(format!("コンポ「{comp}」が見つかりません")))?;
        let index = c.layer_index(layer).ok_or_else(|| err("レイヤーが見つかりません".into()))?;
        with_project(project, |ctx| eval_prop(ctx, c, index, path, t.seconds(), false))
    }

    /// コンポ内の全エクスプレッションを時刻 `t` で検査し、エラーを列挙する（UI の警告表示用）。
    pub fn check(&self, project: &Project, comp: &CompId, t: Time) -> Vec<ExprError> {
        let Some(c) = project.comp(comp) else { return Vec::new() };
        let mut errors = Vec::new();
        for (i, layer) in c.layers.iter().enumerate() {
            let mut paths = Vec::new();
            visit_layer(layer, &mut |p, r| {
                if r.has_expression() && r.dims() > 0 {
                    paths.push(p.to_string());
                }
            });
            for path in paths {
                if let Err(e) = with_project(project, |ctx| eval_prop(ctx, c, i, &path, t.seconds(), false)) {
                    errors.push(e);
                }
            }
        }
        errors
    }

    fn resolve_uncached(&self, project: &Project, comp: &Composition, t: Time) -> Result<Option<Arc<Composition>>, String> {
        if !comp.layers.iter().any(has_expressions) {
            return Ok(None);
        }
        let mut values: Vec<(usize, String, Vec<f64>)> = Vec::new();
        for (i, layer) in comp.layers.iter().enumerate() {
            let mut paths = Vec::new();
            visit_layer(layer, &mut |p, r| {
                if r.has_expression() && r.dims() > 0 {
                    paths.push(p.to_string());
                }
            });
            for path in paths {
                let v = with_project(project, |ctx| eval_prop(ctx, comp, i, &path, t.seconds(), false)).map_err(|e| e.to_string())?;
                values.push((i, path, v));
            }
        }
        if values.is_empty() {
            return Ok(None);
        }
        let mut out = comp.clone();
        for (i, layer) in out.layers.iter_mut().enumerate() {
            visit_layer_mut(layer, &mut |path, mut m| {
                if let Some((_, _, v)) = values.iter().find(|(li, p, _)| *li == i && p == path) {
                    m.set_constant(v);
                }
            });
        }
        Ok(Some(Arc::new(out)))
    }
}

impl Resolver for ExprEngine {
    fn resolve(&self, project: &Project, comp: &Composition, t: Time) -> Result<Option<Arc<Composition>>, String> {
        let key = (project as *const Project as usize, comp.id.clone(), t);
        if let Some(hit) = self.cache.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let result = self.resolve_uncached(project, comp, t);
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, result.clone());
        result
    }

    fn selector(&self, source: &str, text_index: usize, text_total: usize, selector_value: f64, t: Time) -> f64 {
        // セレクターはプロジェクトを参照しないので、空のプロジェクトで評価する
        let empty = Project::new("");
        with_project(&empty, |ctx| {
            let f: Function = ctx.globals().get("__evalSelector")?;
            let v: Value = f.call((source, text_index as i32, text_total as i32, selector_value, t.seconds()))?;
            Ok(to_numbers(&v).and_then(|n| n.first().copied()).unwrap_or(selector_value))
        })
        .unwrap_or(selector_value)
    }
}
