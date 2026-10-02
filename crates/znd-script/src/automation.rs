//! 自動化スクリプト。プロジェクト全体を JavaScript のオブジェクトとして渡し、書き換えた結果を取り込む。
//!
//! プロジェクトファイル（JSON）と同じ構造なので、ファイル形式の知識がそのまま使える。
//! 実行後に構造を検査し、参照切れや循環があれば変更を取り消す。

use rquickjs::{Context, Function, Runtime, Value};
use std::cell::RefCell;
use std::rc::Rc;
use znd_core::Project;

const HELPERS: &str = r#"
var FLICKS = 705600000;
/** 秒 → プロジェクト内部の時刻（flicks） */
function flicks(seconds) { return Math.round(seconds * FLICKS); }
/** プロジェクト内部の時刻 → 秒 */
function seconds(t) { return t / FLICKS; }
/** 名前または ID でコンポを探す */
function comp(nameOrId) {
  for (var i = 0; i < project.compositions.length; i++) {
    var c = project.compositions[i];
    if (c.id === nameOrId || c.name === nameOrId) return c;
  }
  throw new Error('コンポ「' + nameOrId + '」が見つかりません');
}
/** コンポ内のレイヤーを名前または ID で探す */
function layer(c, nameOrId) {
  if (typeof c === 'string') c = comp(c);
  for (var i = 0; i < c.layers.length; i++) {
    if (c.layers[i].id === nameOrId || c.layers[i].name === nameOrId) return c.layers[i];
  }
  throw new Error('レイヤー「' + nameOrId + '」が見つかりません');
}
/** 全コンポの全レイヤーに関数を適用する */
function forEachLayer(fn) {
  project.compositions.forEach(function (c) { c.layers.forEach(function (l, i) { fn(l, c, i); }); });
}
/** プロジェクト内で未使用の ID を作る */
function newId(prefix) {
  var used = {};
  project.footage.forEach(function (f) { used[f.id] = true; });
  project.compositions.forEach(function (c) { used[c.id] = true; c.layers.forEach(function (l) { used[l.id] = true; }); });
  for (var n = 1; ; n++) { var id = prefix + '-' + n; if (!used[id]) return id; }
}
/** キーフレームを作る。kind: 'linear' | 'ease' | 'hold' */
function key(sec, value, kind) {
  var k = { time: flicks(sec), value: value, interp_in: 'linear', interp_out: 'linear', bezier_mode: 'manual' };
  if (kind === 'ease') { k.interp_in = 'bezier'; k.interp_out = 'bezier'; }
  if (kind === 'hold') { k.interp_out = 'hold'; }
  return k;
}
/** プロパティにキーフレームを設定する（時刻順に並べ直す） */
function setKeys(prop, keys) {
  prop.keyframes = keys.slice().sort(function (a, b) { return a.time - b.time; });
  return prop;
}
"#;

/// スクリプトを実行する。成功すればプロジェクトを書き換え、`log()` の出力を返す。
pub fn run_script(project: &mut Project, source: &str) -> Result<Vec<String>, String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(512 * 1024 * 1024);
    let context = Context::full(&rt).map_err(|e| e.to_string())?;
    let logs: Rc<RefCell<Vec<String>>> = Rc::default();
    let json = serde_json::to_string(project).map_err(|e| e.to_string())?;

    let result: Result<String, String> = context.with(|ctx| {
        let describe = |ctx: &rquickjs::Ctx| -> String {
            let caught = ctx.catch();
            match caught.as_object().and_then(|o| rquickjs::Exception::from_object(o.clone())) {
                Some(ex) => {
                    let stack = ex.stack().unwrap_or_default();
                    // スクリプト本体のフレームは「eval_script:行:桁」。補助関数より後に評価するので、最後に現れるものが本体側
                    let line = stack.lines().filter(|l| !l.contains(" at comp ") && !l.contains(" at layer ")).find_map(|l| {
                        let rest = l.split("eval_script:").nth(1)?;
                        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok()
                    });
                    match line {
                        Some(n) => format!("{} 行目: {}", n, ex.message().unwrap_or_default()),
                        None => ex.message().unwrap_or_default(),
                    }
                }
                None => format!("{caught:?}"),
            }
        };
        let g = ctx.globals();
        let sink = logs.clone();
        let log = Function::new(ctx.clone(), move |args: rquickjs::function::Rest<Value>| {
            let line: Vec<String> = args
                .0
                .iter()
                .map(|v| match v.as_string() {
                    Some(s) => s.to_string().unwrap_or_default(),
                    None => v
                        .ctx()
                        .json_stringify(v.clone())
                        .ok()
                        .flatten()
                        .and_then(|s| s.to_string().ok())
                        .unwrap_or_else(|| "undefined".into()),
                })
                .collect();
            sink.borrow_mut().push(line.join(" "));
        })
        .map_err(|e| e.to_string())?;
        g.set("log", log).map_err(|e| e.to_string())?;
        let parsed = ctx.json_parse(json).map_err(|_| describe(&ctx))?;
        g.set("project", parsed).map_err(|e| e.to_string())?;
        ctx.eval::<(), _>(HELPERS).map_err(|_| describe(&ctx))?;
        ctx.eval::<Value, _>(source).map_err(|_| describe(&ctx))?;
        let current: Value = g.get("project").map_err(|e| e.to_string())?;
        let out = ctx.json_stringify(current).map_err(|_| describe(&ctx))?.ok_or("project が JSON にできません")?;
        out.to_string().map_err(|e| e.to_string())
    });

    let updated: Project = serde_json::from_str(&result?).map_err(|e| format!("スクリプト実行後のプロジェクトが不正です: {e}"))?;
    let issues = updated.validate();
    if !issues.is_empty() {
        return Err(format!("スクリプト実行後のプロジェクトに問題があるため、変更を取り消しました:\n  {}", issues.join("\n  ")));
    }
    *project = updated;
    let out = logs.borrow().clone();
    Ok(out)
}
