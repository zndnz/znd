//! WebAssembly のプラグイン（サンドボックス）の受け入れテスト。
//!
//! 手で書いた WAT のモジュールで、正しいエフェクトが動くことと、悪いことをするエフェクトがアプリを壊せないことを確かめる:
//! 無限ループ（時間切れで止まる）・トラップ・範囲外のポインタを渡すホスト関数の呼び出し・上限を超えるメモリの確保・
//! WASI（ファイルなど）を使おうとするモジュール（読み込めない）。
//! 時間切れの上限を変えるので、このファイルのテストは 1 つの関数にまとめている。

mod common;

use common::*;
use std::path::PathBuf;
use znd_render::plugins::{self, guard, PluginApi};

const DESC: &str = r#"{"abi":1,"vendor":"znd test","version":"1","license":"MIT","effects":[
{"id":"t.wasm.black","name":"Black","category":"test","flags":3,"params":[{"id":"gain","label":"gain","kind":0,"def":[1,0,0,0],"min":0,"max":2},{"id":"msg","label":"msg","kind":5}]},
{"id":"t.wasm.loop","flags":0,"params":[]},
{"id":"t.wasm.trap","flags":0,"params":[]},
{"id":"t.wasm.badptr","flags":0,"params":[]},
{"id":"t.wasm.grow","flags":0,"params":[]}]}"#;

fn module_wat() -> String {
    let esc: String = DESC.replace('\n', "").chars().map(|c| if c == '"' { "\\\"".to_string() } else { c.to_string() }).collect();
    let len = DESC.replace('\n', "").len();
    let packed = (1024u64 << 32) | len as u64;
    format!(
        r#"(module
  (import "znd" "set_output" (func $set_output (param i32 i32 i32 i32 i32) (result i32)))
  (import "znd" "image_read" (func $image_read (param i32 i32 i32) (result i32)))
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 65536))
  (data (i32.const 1024) "{esc}")
  (func (export "znd_abi") (result i32) (i32.const 1))
  (func (export "znd_describe") (result i64) (i64.const {packed}))
  (func (export "znd_alloc") (param $n i32) (result i32) (local $p i32) (local $end i32)
    (local.set $p (i32.and (i32.add (global.get $heap) (i32.const 7)) (i32.const -8)))
    (local.set $end (i32.add (local.get $p) (local.get $n)))
    (block $ok (loop $grow
      (br_if $ok (i32.le_u (local.get $end) (i32.shl (memory.size) (i32.const 16))))
      (if (i32.eq (memory.grow (i32.const 16)) (i32.const -1)) (then (return (i32.const 0))))
      (br $grow)))
    (global.set $heap (local.get $end))
    (local.get $p))
  (func (export "znd_free") (param i32 i32))
  (func (export "znd_render") (param $fx i32) (param $args i32) (result i32)
    (local $rgba i32) (local $n i32) (local $i i32) (local $a i32) (local $gain f64)
    ;; 0: RGB に (1 - gain) を掛ける（gain = 1 で黒）
    (if (i32.eqz (local.get $fx)) (then
      (local.set $rgba (i32.load (local.get $args)))
      (local.set $n (i32.mul (i32.load offset=4 (local.get $args)) (i32.load offset=8 (local.get $args))))
      (local.set $gain (f64.load (i32.load offset=60 (local.get $args))))
      (block $done (loop $px
        (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $a (i32.add (local.get $rgba) (i32.mul (local.get $i) (i32.const 16))))
        (f32.store (local.get $a) (f32.mul (f32.load (local.get $a)) (f32.demote_f64 (f64.sub (f64.const 1) (local.get $gain)))))
        (f32.store offset=4 (local.get $a) (f32.mul (f32.load offset=4 (local.get $a)) (f32.demote_f64 (f64.sub (f64.const 1) (local.get $gain)))))
        (f32.store offset=8 (local.get $a) (f32.mul (f32.load offset=8 (local.get $a)) (f32.demote_f64 (f64.sub (f64.const 1) (local.get $gain)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $px)))
      (return (i32.const 0))))
    ;; 1: 無限ループ
    (if (i32.eq (local.get $fx) (i32.const 1)) (then (loop $forever (br $forever))))
    ;; 2: トラップ
    (if (i32.eq (local.get $fx) (i32.const 2)) (then unreachable))
    ;; 3: メモリの外を指すポインタでホスト関数を呼ぶ。ホストは拒否（0 以外）しなければならない
    (if (i32.eq (local.get $fx) (i32.const 3)) (then
      (if (i32.eqz (call $set_output (i32.const -16) (i32.const 1000) (i32.const 1000) (i32.const 0) (i32.const 0))) (then (return (i32.const 1))))
      (if (i32.eqz (call $image_read (i32.const 0) (i32.const -16) (i32.const 4))) (then (return (i32.const 1))))
      (return (i32.const 0))))
    ;; 4: 上限（3GB）を超えるメモリを確保しようとする。失敗（-1）しなければならない
    (if (i32.eq (local.get $fx) (i32.const 4)) (then
      (if (i32.ne (memory.grow (i32.const 50000)) (i32.const -1)) (then (return (i32.const 1))))
      (return (i32.const 0))))
    (i32.const 1))
)"#
    )
}

/// WASI でファイルを開こうとするモジュール（ホストは WASI を渡さないので、インスタンスを作れない）。
const WASI_WAT: &str = r#"(module
  (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "znd_abi") (result i32) (i32.const 1))
  (func (export "znd_describe") (result i64) (i64.const 0))
  (func (export "znd_alloc") (param i32) (result i32) (i32.const 8))
  (func (export "znd_free") (param i32 i32))
  (func (export "znd_render") (param i32 i32) (result i32) (i32.const 0))
)"#;

fn write_wasm(name: &str, wat: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("znd-wasm-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, wat::parse_str(wat).unwrap()).unwrap();
    path
}

fn layer_with(kind: &str) -> znd_core::Project {
    let mut l = solid("a", 8, 8, [1.0, 0.5, 0.25], 8, 8);
    l.effects.push(fx(kind, 8, 8));
    project(vec![comp("main", 8, 8, vec![l])])
}

#[test]
fn sandboxed_plugins_run_and_cannot_break_the_host() {
    plugins::wasm::set_call_timeout(std::time::Duration::from_millis(500));
    let path = write_wasm("test.wasm", &module_wat());
    let infos = plugins::load_plugin_file(&path).expect("WASM を読み込める");
    assert_eq!(infos.len(), 5);
    let black = infos.iter().find(|i| i.kind == "plugin:t.wasm.black").unwrap();
    assert_eq!((black.api, black.param_count, black.label.as_str()), (PluginApi::Wasm, 2, "Black"));

    // 正しいエフェクト: パラメータが届き、結果が返る
    let base = render(&project(vec![comp("main", 8, 8, vec![solid("a", 8, 8, [1.0, 0.5, 0.25], 8, 8)])]), "main");
    let out = render(&layer_with("plugin:t.wasm.black"), "main");
    assert!(out.pixel(4, 4)[0] < 1e-6 && out.pixel(4, 4)[3] > 0.99, "{:?}", out.pixel(4, 4));
    let mut half = layer_with("plugin:t.wasm.black");
    half.compositions[0].layers[0].effects[0].params.insert("gain".into(), znd_core::ParamValue::Scalar(znd_core::Property::constant(0.5)));
    let h = render(&half, "main");
    assert!((h.pixel(4, 4)[0] - base.pixel(4, 4)[0] * 0.5).abs() < 0.02, "{:?} {:?}", h.pixel(4, 4), base.pixel(4, 4));
    // 並列に描いても（スレッドセーフなエフェクトはインスタンスを増やして）壊れない
    std::thread::scope(|s| {
        for _ in 0..6 {
            s.spawn(|| assert!(render(&layer_with("plugin:t.wasm.black"), "main").pixel(2, 2)[0] < 1e-6));
        }
    });

    // 無限ループ: 時間切れで止まり、元の絵のまま。以後このセッションでは止める
    let started = std::time::Instant::now();
    let looped = render(&layer_with("plugin:t.wasm.loop"), "main");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "時間切れで止まる");
    assert!((looped.pixel(4, 4)[0] - base.pixel(4, 4)[0]).abs() < 1e-6, "元の絵のまま");
    assert!(guard::is_quarantined("plugin:t.wasm.loop"));
    assert!(guard::faults().iter().any(|(k, _)| k == "plugin:t.wasm.loop"));

    // トラップ: 元の絵のまま、アプリは続く
    let trapped = render(&layer_with("plugin:t.wasm.trap"), "main");
    assert!((trapped.pixel(4, 4)[0] - base.pixel(4, 4)[0]).abs() < 1e-6);
    assert!(guard::is_quarantined("plugin:t.wasm.trap"));

    // 範囲外のポインタ・上限を超えるメモリは、ホストが拒否する（拒否されなければエフェクトが 1 を返して失敗になる）
    for kind in ["plugin:t.wasm.badptr", "plugin:t.wasm.grow"] {
        let r = render(&layer_with(kind), "main");
        assert!(!guard::is_quarantined(kind), "{kind}: {:?}", guard::faults());
        assert!(all_finite(&r));
    }

    // WASI を使おうとするモジュールは読み込めない（ファイル・ネットワークへの手段を渡さない）
    let wasi = write_wasm("wasi.wasm", WASI_WAT);
    let err = plugins::load_plugin_file(&wasi).unwrap_err();
    assert!(err.contains("wasi_snapshot_preview1::path_open") && err.contains("サンドボックス"), "{err}");

    // ネイティブのライブラリは、許可しない限り読み込まない
    plugins::set_native_allowed(false);
    let dir = std::env::temp_dir().join(format!("znd-wasm-test-{}", std::process::id()));
    let fake = dir.join(if cfg!(target_os = "macos") { "fake.dylib" } else { "fake.so" });
    std::fs::write(&fake, b"\x7fELF not really").unwrap();
    let err = plugins::load_plugin_file(&fake).unwrap_err();
    assert_eq!(err, plugins::NATIVE_BLOCKED);
}
