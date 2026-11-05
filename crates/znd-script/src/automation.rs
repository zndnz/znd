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
/** 既定値のエフェクトを作ってレイヤーの末尾に足し、そのエフェクトを返す。name を省くと種類の表示名 */
function addEffect(l, kind, name) {
  var fx = JSON.parse(__new_effect(String(kind), l.kind && l.kind.width ? l.kind.width : 100, l.kind && l.kind.height ? l.kind.height : 100));
  if (!fx) throw new Error('エフェクト「' + kind + '」がありません');
  if (name) fx.name = String(name);
  if (!l.effects) l.effects = [];
  l.effects.push(fx);
  return fx;
}
/** コンポの大きさの平面を、ID を付けていちばん上に足して返す。color は sRGB の [r, g, b]（0〜1、省略で白） */
function addSolid(c, name, color) {
  var id = newId('layer');
  var l = JSON.parse(__new_solid(id, String(name || '平面'), c.width, c.height, JSON.stringify(color || [1, 1, 1]), c.duration));
  c.layers.unshift(l);
  return l;
}
/** コンポの中心に置いたテキストレイヤー（中央揃え）を、ID を付けていちばん上に足して返す。size は px（省略で 96） */
function addText(c, name, text, size) {
  var id = newId('layer');
  var l = JSON.parse(__new_text(id, String(name || 'テキスト'), String(text == null ? '' : text), Number(size) || 96, c.width, c.height, c.duration));
  c.layers.unshift(l);
  return l;
}
/** エフェクトのパラメータに値を入れる（数・点・色はキーフレームを消して固定値に。真偽・選択肢・文字列はそのまま） */
function setParam(fx, name, v) {
  var p = fx.params[name];
  if (!p) throw new Error('パラメータ「' + name + '」がありません');
  if (p.t === 'scalar' || p.t === 'point' || p.t === 'color') p.v = { value: v };
  else p.v = v;
  return fx;
}
/** 名前でレイヤーのエフェクトを探す（無ければ null） */
function findEffect(l, name) {
  var list = l.effects || [];
  for (var i = 0; i < list.length; i++) if (list[i].name === name || list[i].kind === name) return list[i];
  return null;
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
    let run = run_plugin(project, source, None, &serde_json::Value::Null)?;
    if let Some(updated) = run.project {
        *project = updated;
    }
    Ok(run.logs)
}

/// プラグインのスクリプトから呼べる、画面への操作。
const HOST: &str = r#"
var __actions = [];
var host = {
  /** レイヤーを選択する（null で選択解除） */
  select: function (layerId) { __actions.push({ type: 'select', layer: layerId == null ? null : String(layerId) }); },
  /** 再生ヘッドをフレーム番号へ動かす */
  setFrame: function (frame) { __actions.push({ type: 'frame', frame: Math.round(frame) }); },
  /** 文字をクリップボードにコピーする */
  copyText: function (text) { __actions.push({ type: 'copy_text', text: String(text) }); },
  /** プロジェクトファイルのあるフォルダを OS のファイルマネージャーで開く（未保存なら何もしない） */
  openProjectFolder: function () { __actions.push({ type: 'open_project_folder' }); },
  /** ワークエリアを start〜end フレーム（end は含まない）にする。引数なしで解除 */
  setWorkArea: function (start, end) { __actions.push(start == null ? { type: 'work_area' } : { type: 'work_area', start: Math.round(start), end: Math.round(end) }); },
  /** 再生ヘッドを秒で動かす */
  setTime: function (sec) { __actions.push({ type: 'time', seconds: Number(sec) }); },
  /** 画面下の状態表示に出す */
  status: function (text) { __actions.push({ type: 'status', text: String(text) }); },
  /** メッセージを小さなウィンドウで出す */
  alert: function (text) { __actions.push({ type: 'alert', text: String(text) }); },
  /** 開いているコンポを切り替える */
  openComp: function (compId) { __actions.push({ type: 'comp', comp: String(compId) }); },
  /** パネルを開く・閉じる（"パッケージID/パネルID"） */
  openPanel: function (key) { __actions.push({ type: 'panel', key: String(key), open: true }); },
  closePanel: function (key) { __actions.push({ type: 'panel', key: String(key), open: false }); },
  /** 出力フィルターを切り替える（"パッケージID/フィルターID"） */
  setFilter: function (key, on) { __actions.push({ type: 'filter', key: String(key), on: !!on }); },
  /** 別のコマンドを実行する（"パッケージID/コマンドID"） */
  runCommand: function (key) { __actions.push({ type: 'command', key: String(key) }); },
  /**
   * ファイルを選んでもらい、中身を受け取る（plugin.json の permissions に "filesystem" が必要）。
   * 選ばれると、同じスクリプトの関数 callback(中身, パス, 追加の値) が呼ばれる。
   * opts: { extensions: ['srt'], binary: false（true なら中身は 0〜255 の数の配列）, title, data（callback にそのまま渡す値） }
   */
  openFile: function (opts, callback) {
    opts = opts || {};
    var name = typeof callback === 'function' ? callback.name : String(callback || '');
    if (!name) throw new Error('host.openFile: callback（関数か関数名）を指定してください');
    __actions.push({ type: 'open_file', extensions: (opts.extensions || []).map(String), binary: !!opts.binary,
      title: opts.title ? String(opts.title) : '', callback: name, data: opts.data === undefined ? null : opts.data });
  },
  /**
   * 保存先を選んでもらい、中身を書き出す（"filesystem" 権限が必要）。
   * content は文字列、または 0〜255 の数の配列（バイナリ）。opts: { extensions: ['srt'] }
   */
  saveFile: function (fileName, content, opts) {
    opts = opts || {};
    var binary = Array.isArray(content);
    __actions.push({ type: 'save_file', name: String(fileName || 'untitled'), binary: binary,
      content: binary ? content.map(function (b) { return b & 255; }) : String(content), extensions: (opts.extensions || []).map(String) });
  },
  /**
   * レイヤーの音の大きさを 1 フレームごとに測り、同じスクリプトの関数 callback(結果, 追加の値) に渡す。
   * 結果は { レイヤーID: [dBFS, ...] }（今のコンポの頭から終わりまでのフレームごと。音量・エフェクトを掛けた後の RMS。無音は -120）。
   * opts: { comp（既定は今のコンポ）, data（callback にそのまま渡す値） }
   */
  audioLevels: function (layerIds, callback, opts) {
    opts = opts || {};
    var name = typeof callback === 'function' ? callback.name : String(callback || '');
    if (!name) throw new Error('host.audioLevels: callback（関数か関数名）を指定してください');
    __actions.push({ type: 'audio_levels', layers: (layerIds || []).map(String), comp: opts.comp ? String(opts.comp) : null,
      callback: name, data: opts.data === undefined ? null : opts.data });
  },
  /** 素材のファイルを選んでもらい、今のコンポの再生ヘッドの位置に読み込む（"filesystem" 権限が必要） */
  importFiles: function (opts) {
    opts = opts || {};
    __actions.push({ type: 'import', extensions: (opts.extensions || []).map(String), sequence: !!opts.sequence });
  }
};
"#;

/// プラグインのスクリプトを 1 回実行した結果。
#[derive(Debug, Clone, Default)]
pub struct PluginRun {
    /// プロジェクトが書き換えられたときだけ、検査済みの新しいプロジェクト。
    pub project: Option<Project>,
    pub logs: Vec<String>,
    /// 呼んだ関数の戻り値（JSON）。
    pub ret: serde_json::Value,
    /// `host.*` で頼まれた画面への操作。
    pub actions: Vec<serde_json::Value>,
}

/// プラグインのスクリプトを実行する。
///
/// グローバルに `project`（プロジェクト全体の JSON。書き換えてよい）、`ctx`（`context` の中身。選択中のレイヤー・時刻など）、
/// `host`（画面への操作）、`log()` と補助関数（`comp` `layer` `key` など）がある。
/// `call` を渡すと、ファイル全体を評価したあと `関数名(引数...)` を呼び、その戻り値を `ret` に入れる。
/// 環境にファイル・ネットワーク・別プログラムへの手段は無い。
pub fn run_plugin(
    project: &Project,
    source: &str,
    call: Option<(&str, &[serde_json::Value])>,
    context: &serde_json::Value,
) -> Result<PluginRun, String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(512 * 1024 * 1024);
    rt.set_max_stack_size(1024 * 1024);
    // 無限ループで画面が固まらないよう、実行時間に上限を設ける
    let started = std::time::Instant::now();
    rt.set_interrupt_handler(Some(Box::new(move || started.elapsed() > std::time::Duration::from_secs(10))));
    let ctx_full = Context::full(&rt).map_err(|e| e.to_string())?;
    let logs: Rc<RefCell<Vec<String>>> = Rc::default();
    let json = serde_json::to_string(project).map_err(|e| e.to_string())?;
    let context_json = serde_json::to_string(context).map_err(|e| e.to_string())?;
    let args_json = serde_json::to_string(&call.map(|(_, a)| a.to_vec()).unwrap_or_default()).map_err(|e| e.to_string())?;

    let result: Result<(String, String, String), String> = ctx_full.with(|ctx| {
        let describe = |ctx: &rquickjs::Ctx| -> String {
            if started.elapsed() > std::time::Duration::from_secs(10) {
                return "実行時間が 10 秒を超えたため止めました".into();
            }
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
        // 既定値のエフェクト（ファイル形式と同じ JSON）。種類が無ければ "null"
        let new_effect = Function::new(ctx.clone(), |kind: String, w: f64, h: f64| -> String {
            match znd_core::effect::EffectInstance::new(&kind, w, h) {
                Some(fx) => serde_json::to_string(&fx).unwrap_or_else(|_| "null".into()),
                None => "null".into(),
            }
        })
        .map_err(|e| e.to_string())?;
        g.set("__new_effect", new_effect).map_err(|e| e.to_string())?;
        // コンポの大きさの平面レイヤー（ファイル形式と同じ JSON）
        let new_solid = Function::new(ctx.clone(), |id: String, name: String, w: u32, h: u32, color: String, dur: f64| -> String {
            let c: [f32; 3] = serde_json::from_str(&color).unwrap_or([1.0, 1.0, 1.0]);
            let l = znd_core::Layer::new(
                id.as_str(),
                name.as_str(),
                znd_core::LayerKind::Solid { width: w.max(1), height: h.max(1), color: c },
                znd_core::Transform::centered(w as f64, h as f64, w as f64, h as f64),
                znd_core::Time(dur as i64),
            );
            serde_json::to_string(&l).unwrap_or_else(|_| "null".into())
        })
        .map_err(|e| e.to_string())?;
        g.set("__new_solid", new_solid).map_err(|e| e.to_string())?;
        // コンポの中心に置いたテキストレイヤー（アプリの「テキスト追加」と同じ作り）
        let new_text = Function::new(ctx.clone(), |id: String, name: String, text: String, size: f64, w: u32, h: u32, dur: f64| -> String {
            let mut doc = znd_core::text::TextDocument::new(&text, size);
            doc.justify = znd_core::text::Justify::Center;
            let l = znd_core::Layer::new(
                id.as_str(),
                name.as_str(),
                znd_core::LayerKind::Text { document: doc, animators: Vec::new(), path: None, per_char_3d: false, text_expression: None },
                znd_core::Transform::centered(0.0, 0.0, w as f64, h as f64),
                znd_core::Time(dur as i64),
            );
            serde_json::to_string(&l).unwrap_or_else(|_| "null".into())
        })
        .map_err(|e| e.to_string())?;
        g.set("__new_text", new_text).map_err(|e| e.to_string())?;
        let parsed = ctx.json_parse(json.clone()).map_err(|_| describe(&ctx))?;
        g.set("project", parsed).map_err(|e| e.to_string())?;
        let parsed_ctx = ctx.json_parse(context_json).map_err(|_| describe(&ctx))?;
        g.set("ctx", parsed_ctx).map_err(|e| e.to_string())?;
        ctx.eval::<(), _>(HELPERS).map_err(|_| describe(&ctx))?;
        ctx.eval::<(), _>(HOST).map_err(|_| describe(&ctx))?;
        ctx.eval::<Value, _>(source).map_err(|_| describe(&ctx))?;
        let ret: Value = if let Some((name, _)) = call {
            let f: Value = g.get(name).map_err(|e| e.to_string())?;
            let Some(f) = f.as_function() else { return Err(format!("関数 {name} がありません")) };
            let args: Value = ctx.json_parse(args_json).map_err(|_| describe(&ctx))?;
            let list: Vec<Value> = args.as_array().map(|a| a.iter::<Value>().filter_map(|v| v.ok()).collect()).unwrap_or_default();
            let mut a = rquickjs::function::Args::new(ctx.clone(), list.len());
            for v in list {
                a.push_arg(v).map_err(|e| e.to_string())?;
            }
            f.call_arg(a).map_err(|_| describe(&ctx))?
        } else {
            Value::new_undefined(ctx.clone())
        };
        fn stringify<'js>(ctx: &rquickjs::Ctx<'js>, v: Value<'js>) -> String {
            ctx.json_stringify(v).ok().flatten().and_then(|s| s.to_string().ok()).unwrap_or_else(|| "null".into())
        }
        let current: Value = g.get("project").map_err(|e| e.to_string())?;
        let out = ctx.json_stringify(current).map_err(|_| describe(&ctx))?.ok_or("project が JSON にできません")?;
        let actions: Value = g.get("__actions").map_err(|e| e.to_string())?;
        Ok((out.to_string().map_err(|e| e.to_string())?, stringify(&ctx, ret), stringify(&ctx, actions)))
    });
    let (out, ret, actions) = result?;
    let logs = logs.borrow().clone();
    let ret = serde_json::from_str(&ret).unwrap_or(serde_json::Value::Null);
    let actions = match serde_json::from_str(&actions) {
        Ok(serde_json::Value::Array(a)) => a,
        _ => Vec::new(),
    };
    // 書き換えが無ければ、検査も取り消し単位の追加もしない
    let before: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let after: serde_json::Value = serde_json::from_str(&out).map_err(|e| format!("スクリプト実行後のプロジェクトが不正です: {e}"))?;
    if before == after {
        return Ok(PluginRun { project: None, logs, ret, actions });
    }
    let updated: Project = serde_json::from_value(after).map_err(|e| format!("スクリプト実行後のプロジェクトが不正です: {e}"))?;
    let issues = updated.validate();
    if !issues.is_empty() {
        return Err(format!("スクリプト実行後のプロジェクトに問題があるため、変更を取り消しました:\n  {}", issues.join("\n  ")));
    }
    Ok(PluginRun { project: Some(updated), logs, ret, actions })
}
