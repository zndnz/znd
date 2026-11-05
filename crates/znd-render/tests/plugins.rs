//! プラグイン機能の受け入れテスト。
//!
//! サンプルプラグイン（examples/plugins）と検証用の OpenFX プラグイン（tests/ofx）を、その場で
//! コンパイルして読み込む。C コンパイラ（cc）が無い環境ではスキップする。
//!
//! 環境変数 `ZND_OFX_TEST_DIR` に OpenFX 公式リポジトリのサンプル（Basic / Invert / ChoiceParams）を
//! ビルドした `.ofx.bundle` を置いたフォルダを指定すると、それらも読み込んで描画する。

mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use znd_core::effect::{effect_def, Pd};
use znd_core::*;
use znd_render::color::{linear_to_srgb, srgb_to_linear};
use znd_render::plugins::{self, PluginApi};
use znd_render::RenderOptions;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn out_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("znd-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    })
}

fn lib_ext() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    }
}

fn run(cmd: &mut Command) -> bool {
    match cmd.output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            eprintln!("{}", String::from_utf8_lossy(&o.stderr));
            false
        }
        Err(_) => false,
    }
}

/// サンプルの C プラグインをコンパイルして読み込む（1 回だけ）。
fn native_c() -> Option<&'static PathBuf> {
    static LIB: OnceLock<Option<PathBuf>> = OnceLock::new();
    LIB.get_or_init(|| {
        let out = out_dir().join(format!("vignette.{}", lib_ext()));
        let ok = run(Command::new("cc")
            .args(["-O1", "-shared", "-fPIC", "-Wall", "-Werror", "-I"])
            .arg(root().join("sdk"))
            .arg("-o")
            .arg(&out)
            .arg(root().join("examples/plugins/vignette.c")));
        if !ok {
            eprintln!("cc が使えないためスキップ");
            return None;
        }
        plugins::load_plugin_file(&out).unwrap();
        Some(out)
    })
    .as_ref()
}

/// 検証用の OpenFX プラグインをバンドルの形にして読み込む（1 回だけ）。
fn ofx_bundle() -> Option<&'static PathBuf> {
    static BUNDLE: OnceLock<Option<PathBuf>> = OnceLock::new();
    BUNDLE
        .get_or_init(|| {
            let arch = if cfg!(target_os = "macos") {
                "MacOS"
            } else if cfg!(target_arch = "aarch64") {
                "Linux-aarch64"
            } else {
                "Linux-x86-64"
            };
            let bundle = out_dir().join("ofx").join("ZndTest.ofx.bundle");
            let bin_dir = bundle.join("Contents").join(arch);
            std::fs::create_dir_all(&bin_dir).unwrap();
            let src = root().join("crates/znd-render/tests/ofx");
            let ok = run(Command::new("cc")
                .args(["-O1", "-shared", "-fPIC", "-Wall", "-I"])
                .arg(src.join("include"))
                .arg("-o")
                .arg(bin_dir.join("ZndTest.ofx"))
                .arg(src.join("znd_test_fx.c")));
            if !ok {
                eprintln!("cc が使えないためスキップ");
                return None;
            }
            // フォルダの走査で見つかること
            let report = plugins::scan(&[out_dir().join("ofx")]);
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            Some(bundle)
        })
        .as_ref()
}

/// `color`（sRGB 表示値）の平面 1 枚に `fx` を掛けたコンポ。
fn one_layer(w: u32, h: u32, color: [f32; 3], effects: Vec<EffectInstance>) -> Project {
    let mut l = solid("a", w, h, color, w, h);
    l.effects = effects;
    project(vec![comp("main", w, h, vec![l])])
}

fn close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} != {b}");
}

#[test]
fn p1_native_plugin_is_registered_with_typed_parameters() {
    let Some(lib) = native_c() else { return };
    let infos = plugins::loaded();
    let v = infos.iter().find(|i| i.kind == "plugin:org.znd.example.vignette").expect("vignette is loaded");
    assert_eq!((v.label.as_str(), v.category.as_str(), v.api, v.param_count), ("周辺減光", "サンプル", PluginApi::Native, 5));
    assert_eq!((v.vendor.as_str(), v.version.as_str(), v.license.as_str()), ("znd サンプル", "1.0", "MIT OR Apache-2.0"));
    assert_eq!(&v.path, lib);
    // 組み込みエフェクトと同じ定義表に入り、パラメータに型と範囲が付く
    let def = effect_def("plugin:org.znd.example.vignette").unwrap();
    assert_eq!(def.params.iter().map(|p| p.name).collect::<Vec<_>>(), ["amount", "radius", "center", "shape", "invert"]);
    assert!(matches!(def.params[0].kind, Pd::F(d, lo, hi) if (d, lo, hi) == (0.6, 0.0, 1.0)));
    assert!(matches!(def.params[2].kind, Pd::P(x, y) if (x, y) == (0.5, 0.5)));
    assert!(matches!(&def.params[3].kind, Pd::I(0, labels) if *labels == ["円", "ひし形"]));
    assert!(matches!(def.params[4].kind, Pd::B(false)));
    // 同じファイルを読み直しても増えない
    assert!(plugins::load_plugin_file(lib).unwrap().is_empty());
    assert_eq!(plugins::loaded().iter().filter(|i| i.kind == v.kind).count(), 1);
}

#[test]
fn p2_native_plugin_renders_with_scaled_and_keyframed_parameters() {
    if native_c().is_none() {
        return;
    }
    // 色かぶり: 白に (1, 0.5, 0) を 50% 混ぜる。色はリニアに直して渡される
    let tint = fx("plugin:org.znd.example.tint", 100, 100);
    let buf = render(&one_layer(100, 100, WHITE, vec![tint]), "main");
    let px = buf.pixel(50, 50);
    close(px[0], 1.0, 1e-4, "r");
    close(px[1], (1.0 + srgb_to_linear(0.5)) / 2.0, 1e-4, "g");
    close(px[2], 0.5, 1e-4, "b");

    // 周辺減光: 半径 50px、中心は既定（レイヤー中央）
    let vig = fx("plugin:org.znd.example.vignette", 100, 100).set_f("amount", 1.0).set_f("radius", 50.0);
    let p = one_layer(100, 100, WHITE, vec![vig.clone()]);
    let full = render(&p, "main");
    close(full.pixel(0, 0)[0], 0.0, 1e-5, "角は真っ黒");
    close(full.pixel(74, 49)[0], 1.0 - 0.49f32.powi(2) - 0.0001, 0.02, "半径の半分の位置");
    assert!(full.pixel(50, 50)[0] > 0.99);
    // 1/2 解像度でも見た目が同じ（px 単位のパラメータと位置に縮尺が掛かる）
    let half = render_at(&p, "main", 0.0, RenderOptions { downsample: 2, ..ALPHA });
    close(half.pixel(37, 25)[0], 0.75, 0.02, "1/2 解像度");
    close(half.pixel(0, 0)[0], 0.0, 1e-5, "1/2 解像度の角");

    // キーフレーム: ホストが評価した値が渡る
    let amount = Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 0.0), Keyframe::linear(Time::from_seconds(1.0), 1.0)]);
    let p = one_layer(100, 100, WHITE, vec![vig.set_prop("amount", amount)]);
    close(render_at(&p, "main", 0.0, ALPHA).pixel(0, 0)[0], 1.0, 1e-5, "t=0");
    close(render_at(&p, "main", 0.5, ALPHA).pixel(0, 0)[0], 0.5, 1e-3, "t=0.5");
}

#[test]
fn p3_plugin_written_in_rust_and_time_dependence() {
    let out = out_dir().join(format!("scanline.{}", lib_ext()));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let built = run(Command::new(rustc)
        .args(["--crate-type", "cdylib", "-O", "--edition", "2021", "-o"])
        .arg(&out)
        .arg(root().join("examples/plugins/scanline.rs")));
    if !built {
        eprintln!("rustc が使えないためスキップ");
        return;
    }
    let infos = plugins::load_plugin_file(&out).unwrap();
    assert_eq!(infos.len(), 1);
    let kind = "plugin:org.znd.example.scanline";
    assert!(effect_def(kind).unwrap().time_dependent);
    let line = fx(kind, 64, 64).set_f("spacing", 8.0).set_f("darkness", 1.0).set_f("speed", 30.0);
    let p = one_layer(64, 64, WHITE, vec![line]);
    let a = render_at(&p, "main", 0.0, ALPHA);
    // 8px 周期の前半 4px が暗い
    assert_eq!((a.pixel(10, 0)[0], a.pixel(10, 4)[0], a.pixel(10, 8)[0]), (0.0, 1.0, 0.0));
    // 0.1 秒で 3px 流れる
    let b = render_at(&p, "main", 0.1, ALPHA);
    assert_eq!((b.pixel(10, 0)[0], b.pixel(10, 1)[0], b.pixel(10, 5)[0]), (0.0, 1.0, 0.0));
    // 時刻に依存するエフェクトは、フレームごとに指紋が変わる（キャッシュを使い回さない）
    let f = znd_render::fingerprint::Fingerprinter::new(&p);
    let id = CompId::new("main");
    assert_ne!(f.frame(&p, &id, Time::ZERO, ALPHA), f.frame(&p, &id, Time::from_seconds(0.1), ALPHA));
}

#[test]
fn p4_broken_and_missing_plugins_do_not_break_rendering() {
    let dir = out_dir().join("broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("garbage.{}", lib_ext())), b"this is not a library").unwrap();
    std::fs::write(dir.join("readme.txt"), b"ignored").unwrap();
    let report = plugins::scan(&[dir.clone(), dir.join("does-not-exist")]);
    assert!(report.loaded.is_empty());
    assert_eq!(report.errors.len(), 1);
    assert!(report.errors[0].1.contains("読み込めません"), "{:?}", report.errors);
    assert!(plugins::load_errors().iter().any(|(p, _)| p.ends_with(format!("garbage.{}", lib_ext()))));

    // プラグインが無い環境で開いたプロジェクト: パラメータは保持され、絵は元のまま、検出できる
    let mut gone = EffectInstance::new("gaussian_blur", 10.0, 10.0).unwrap();
    gone.kind = "plugin:com.example.not-installed".into();
    gone.name = "入っていないプラグイン".into();
    let p = one_layer(16, 16, RED, vec![gone]);
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<Project>(&json).unwrap(), p);
    let buf = render(&p, "main");
    close(buf.pixel(8, 8)[0], 1.0, 1e-6, "元のまま");
    let missing = plugins::missing(&p);
    assert_eq!(missing, vec![("plugin:com.example.not-installed".to_string(), "main / a".to_string())]);
}

#[test]
fn o1_openfx_plugin_is_described_and_parameters_are_mapped() {
    let Some(bundle) = ofx_bundle() else { return };
    let infos: Vec<_> = plugins::loaded().into_iter().filter(|i| i.path == *bundle).collect();
    let kinds: Vec<&str> = infos.iter().map(|i| i.kind.as_str()).collect();
    // ジェネレーター専用のものは読み込まれない
    assert_eq!(kinds, ["ofx:org.znd.test.bytes", "ofx:org.znd.test.echo", "ofx:org.znd.test.gain"]);
    let gain = infos.iter().find(|i| i.kind.ends_with("gain")).unwrap();
    assert_eq!(
        (gain.label.as_str(), gain.category.as_str(), gain.api, gain.version.as_str()),
        ("テスト: ゲイン", "OpenFX / znd テスト", PluginApi::OpenFx, "1.2")
    );

    let def = effect_def("ofx:org.znd.test.gain").unwrap();
    let names: Vec<&str> = def.params.iter().map(|p| p.name).collect();
    // グループと隠しパラメータは出ない。2 成分の整数は成分ごとに分かれる
    assert_eq!(names, ["gain", "tint", "invert", "channel", "marker", "shift.x", "shift.y", "note"]);
    assert_eq!(def.params[0].label, "ゲイン");
    assert!(matches!(def.params[0].kind, Pd::F(d, lo, hi) if (d, lo, hi) == (1.0, 0.0, 4.0)));
    assert!(matches!(def.params[1].kind, Pd::C(c) if c == [1.0; 4]));
    assert!(matches!(def.params[2].kind, Pd::B(false)));
    assert!(matches!(&def.params[3].kind, Pd::I(0, labels) if *labels == ["すべて", "赤だけ"]));
    assert!(matches!(def.params[4].kind, Pd::P(_, _)));
    assert!(matches!(def.params[7].kind, Pd::S));
    // 別時刻の入力を使うプラグインは、時刻依存として扱う
    assert!(effect_def("ofx:org.znd.test.echo").unwrap().time_dependent);
    assert!(!def.time_dependent);
}

#[test]
fn o2_openfx_render_passes_images_and_parameters() {
    if ofx_bundle().is_none() {
        return;
    }
    let gray = [0.5f32; 3];
    let lin = srgb_to_linear(0.5);
    let gain = |f: &dyn Fn(EffectInstance) -> EffectInstance| one_layer(40, 30, gray, vec![f(fx("ofx:org.znd.test.gain", 40, 30))]);

    // 既定値では何も変わらない（目印は左下 = レイヤー座標 (0, 高さ) なので画面外）
    let p = gain(&|e| e.set_p("marker", [-10.0, -10.0]));
    let buf = render(&p, "main");
    close(buf.pixel(20, 15)[0], lin, 1e-6, "既定値");
    close(buf.pixel(20, 15)[3], 1.0, 1e-6, "アルファ");

    // Double / RGBA / Boolean / Choice
    let p = gain(&|e| e.set_p("marker", [-10.0, -10.0]).set_f("gain", 2.0).set_c("tint", [1.0, 0.0, 0.0, 1.0]));
    let px = render(&p, "main").pixel(20, 15);
    close(px[0], lin * 2.0, 1e-5, "gain × tint の赤");
    close(px[1], 0.0, 1e-6, "tint の緑");
    let p = gain(&|e| e.set_p("marker", [-10.0, -10.0]).set_b("invert", true).set_i("channel", 1));
    let px = render(&p, "main").pixel(20, 15);
    close(px[0], 1.0 - lin, 1e-5, "反転した赤");
    close(px[1], lin, 1e-6, "赤だけなので緑はそのまま");

    // 位置パラメータ: レイヤー座標（左上原点）の (10, 6) が、絵の同じ位置に出る（上下が入れ替わっていない）
    let p = gain(&|e| e.set_p("marker", [10.5, 6.5]));
    let buf = render(&p, "main");
    assert_eq!(buf.pixel(10, 6), [1.0, 0.0, 1.0, 1.0]);
    close(buf.pixel(10, 23)[1], lin, 1e-6, "上下反対の位置には出ない");
    // 1/2 解像度では描画スケールが渡り、同じ場所（半分の座標）に出る
    let half = render_at(&p, "main", 0.0, RenderOptions { downsample: 2, ..ALPHA });
    assert_eq!(half.pixel(5, 3), [1.0, 0.0, 1.0, 1.0]);

    // Integer2D: 右へ 3px ずらす → 左端 3 列が透明になる
    let p = gain(&|e| e.set_p("marker", [-10.0, -10.0]).set_f("shift.x", 3.0));
    let buf = render(&p, "main");
    assert_eq!(buf.pixel(2, 15)[3], 0.0);
    close(buf.pixel(3, 15)[3], 1.0, 1e-6, "ずらした先");

    // String とメッセージスイート
    let p = gain(&|e| e.set_p("marker", [-10.0, -10.0]).set_f("gain", 2.0).set_s("note", "msg"));
    render(&p, "main");
    let messages = znd_render::plugins::ofx::recent_messages();
    assert!(messages.iter().any(|m| m.contains("gain=2.00 note=msg")), "{messages:?}");
}

#[test]
fn o3_openfx_temporal_access_and_integer_pixel_formats() {
    if ofx_bundle().is_none() {
        return;
    }
    // エコー: 出力 = 入力 × level(t) ＋ 1 フレーム前の入力 × level(t − 1 フレーム)
    let level = Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 0.0), Keyframe::linear(Time::from_seconds(1.0), 1.0)]);
    let p = one_layer(16, 16, [0.5; 3], vec![fx("ofx:org.znd.test.echo", 16, 16).set_prop("level", level)]);
    let lin = srgb_to_linear(0.5);
    let buf = render_at(&p, "main", 0.5, ALPHA);
    close(buf.pixel(8, 8)[0], lin * (15.0 / 30.0 + 14.0 / 30.0), 1e-5, "現在と 1 フレーム前の値");

    // 8bit しか扱えないプラグイン: ホストが表示値の 8bit に変換して渡し、結果を戻す
    let p = one_layer(16, 16, [200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0], vec![fx("ofx:org.znd.test.bytes", 16, 16)]);
    let px = render(&p, "main").pixel(8, 8);
    let shown = [px[0], px[1], px[2]].map(|v| (linear_to_srgb(v) * 255.0).round() as i32);
    assert_eq!(shown, [50, 100, 200]);
}

/// OpenFX 公式リポジトリのサンプルプラグイン（任意）。
#[test]
fn o4_official_openfx_examples() {
    let Some(dir) = std::env::var_os("ZND_OFX_TEST_DIR").map(PathBuf::from) else {
        eprintln!("ZND_OFX_TEST_DIR が未設定のためスキップ");
        return;
    };
    let report = plugins::scan(&[dir.clone()]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let ours: Vec<_> = plugins::loaded().into_iter().filter(|i| i.path.starts_with(&dir)).collect();
    for i in &ours {
        let def = effect_def(&i.kind).unwrap();
        eprintln!("{} 「{}」 [{}] params: {:?}", i.kind, i.label, i.category, def.params.iter().map(|p| p.name).collect::<Vec<_>>());
    }
    let has = |id: &str| ours.iter().any(|i| i.kind == id);
    let gray = [0.25f32; 3];
    let run = |kind: &str, f: &dyn Fn(EffectInstance) -> EffectInstance| -> [f32; 4] {
        render(&one_layer(32, 24, gray, vec![f(fx(kind, 32, 24))]), "main").pixel(16, 12)
    };
    let shown = |px: [f32; 4]| [px[0], px[1], px[2]].map(|v| (linear_to_srgb(v) * 255.0).round() as i32);

    // Invert（8bit 専用）: 255 − 値
    if has("ofx:uk.co.thefoundry.OfxInvertExample") {
        let px = run("ofx:uk.co.thefoundry.OfxInvertExample", &|e| e);
        assert_eq!(
            shown([px[0] / px[3].max(1e-6), px[1] / px[3].max(1e-6), px[2] / px[3].max(1e-6), 1.0])[0],
            0,
            "アルファも反転するので 0 になる: {px:?}"
        );
    }
    // Basic（ゲイン）: scale を 2 にすると 2 倍
    if has("ofx:uk.co.thefoundry.BasicGainPlugin") {
        let before = run("ofx:uk.co.thefoundry.BasicGainPlugin", &|e| e);
        let after = run("ofx:uk.co.thefoundry.BasicGainPlugin", &|e| e.set_f("scale", 2.0));
        close(before[0], srgb_to_linear(0.25), 1e-5, "Basic 既定値");
        close(after[0], srgb_to_linear(0.25) * 2.0, 1e-5, "Basic scale=2");
        // このサンプルはアルファにも同じ倍率を掛ける。ホストは 0..1 に収める
        close(after[3], 1.0, 1e-5, "アルファは 1 に収まる");
    }
    if has("ofx:org.openeffects:ChoiceParamsExample") || ours.iter().any(|i| i.kind.contains("Choice")) {
        let kind = ours.iter().find(|i| i.kind.contains("Choice")).unwrap().kind.clone();
        let px = run(&kind, &|e| e);
        assert!(px.iter().all(|v| v.is_finite()));
    }
    // C++ サポートライブラリで書かれたサンプル
    if has("ofx:net.sf.openfx.invertPlugin") {
        let px = run("ofx:net.sf.openfx.invertPlugin", &|e| e);
        eprintln!("Support Invert: {px:?}");
        close(px[0], 1.0 - srgb_to_linear(0.25), 1e-5, "Support Invert（float で 1 − 値）");
    }
    if has("ofx:net.sf.openfx.retimer") {
        // 速度 1 なら同じ時刻の入力がそのまま出る（別時刻の入力を取りに来るプラグイン）
        let px = render_at(&one_layer(32, 24, gray, vec![fx("ofx:net.sf.openfx.retimer", 32, 24)]), "main", 0.5, ALPHA).pixel(16, 12);
        eprintln!("Retimer: {px:?}");
        close(px[0], srgb_to_linear(0.25), 1e-4, "Retimer");
    }
    assert!(!ours.is_empty(), "公式サンプルが 1 つも読み込めていない");
}

/// ホストが使う OpenFX の文字列定数が、公式ヘッダーの定義と一致していること
/// （値を 1 文字でも間違えると、プラグイン側で例外になって描画に失敗する）。
#[test]
fn o5_openfx_string_constants_match_official_headers() {
    let host = std::fs::read_to_string(root().join("crates/znd-render/src/plugins/ofx.rs")).unwrap();
    let mut defined = std::collections::HashSet::new();
    for entry in std::fs::read_dir(root().join("crates/znd-render/tests/ofx/include")).unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "h") {
            for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
                let line = line.trim_start();
                if let Some(rest) = line.strip_prefix("#define ") {
                    if let (Some(a), Some(b)) = (rest.find('"'), rest.rfind('"')) {
                        if b > a {
                            defined.insert(rest[a + 1..b].to_string());
                        }
                    }
                }
            }
        }
    }
    // ヘッダーの定数ではないもの（関数名、接頭辞として使う文字列、ホスト独自の型名）
    let allowed = ["OfxGetNumberOfPlugins", "OfxGetPlugin", "OfxMessage", "OfxParamType", "OfxTypeParameterSet"];
    let mut unknown = Vec::new();
    let mut rest = host.as_str();
    while let Some(i) = rest.find("\"Ofx").or_else(|| rest.find("\"kOfx")) {
        let tail = &rest[i + 1..];
        let end = tail.find('"').unwrap();
        let lit = &tail[..end];
        if lit.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !defined.contains(lit) && !allowed.contains(&lit) {
            unknown.push(lit.to_string());
        }
        rest = &tail[end + 1..];
    }
    unknown.sort();
    unknown.dedup();
    assert!(unknown.is_empty(), "公式ヘッダーに無い定数: {unknown:?}");
}

/// ABI 3 のサンプル（出力の大きさの変更・組み込みエフェクトの呼び出し・音声エフェクト）を読み込む。
fn native_abi3() -> Option<&'static PathBuf> {
    static LIB: OnceLock<Option<PathBuf>> = OnceLock::new();
    LIB.get_or_init(|| {
        let out = out_dir().join(format!("outline.{}", lib_ext()));
        let ok = run(Command::new("cc")
            .args(["-O1", "-shared", "-fPIC", "-Wall", "-Werror", "-I"])
            .arg(root().join("sdk"))
            .arg("-o")
            .arg(&out)
            .arg(root().join("examples/plugins/outline.c"))
            .arg("-lm"));
        if !ok {
            eprintln!("cc が使えないためスキップ");
            return None;
        }
        plugins::load_plugin_file(&out).unwrap();
        Some(out)
    })
    .as_ref()
}

#[test]
fn p5_abi3_effect_can_grow_the_layer_image() {
    if native_abi3().is_none() {
        return;
    }
    // 20x20 の赤い平面を 60x60 のコンポの中央に置き、5px の白い縁を付ける
    let mut l = solid("a", 20, 20, RED, 60, 60);
    l.effects = vec![fx("plugin:org.znd.example.outline", 20, 20).set_f("width", 5.0).set_c("color", [1.0, 1.0, 1.0, 1.0])];
    let p = project(vec![comp("main", 60, 60, vec![l])]);
    let buf = render(&p, "main");
    // 中は赤のまま、元の画像の外側 3px の位置は白い縁、10px 離れた位置は透明
    let c = buf.pixel(30, 30);
    close(c[0], 1.0, 1e-4, "中の赤");
    close(c[1], 0.0, 1e-4, "中の緑");
    let edge = buf.pixel(30, 17);
    close(edge[1], 1.0, 1e-4, "上の縁");
    close(edge[3], 1.0, 1e-4, "上の縁のアルファ");
    close(buf.pixel(42, 30)[1], 1.0, 1e-4, "右の縁");
    close(buf.pixel(30, 9)[3], 0.0, 1e-6, "縁の外");
    // 1/2 解像度でも同じ位置に縁が付く
    let half = render_at(&p, "main", 0.0, RenderOptions { downsample: 2, ..ALPHA });
    close(half.pixel(15, 8)[3], 1.0, 0.05, "1/2 解像度の縁");
    close(half.pixel(15, 4)[3], 0.0, 1e-6, "1/2 解像度の縁の外");
}

#[test]
fn p6_abi3_audio_effect_is_registered_as_audio_processor() {
    if native_abi3().is_none() {
        return;
    }
    let kind = "plugin:org.znd.example.gain";
    let proc = znd_core::effect::audio_processor(kind).expect("音声エフェクトとして登録される");
    let fx = fx(kind, 10, 10).set_f("gain", 0.25);
    let mut samples = vec![1.0f32; 64];
    proc.process(
        &fx.eval(Time::ZERO),
        znd_core::effect::AudioBlock { samples: &mut samples, sample_rate: 48000, start: 0, time: Time::ZERO, instance: 1, seed: 0, length: 0 },
    );
    assert!(samples.iter().all(|v| (*v - 0.25).abs() < 1e-6));
    // 映像には何もしない
    let p = one_layer(8, 8, RED, vec![fx]);
    close(render(&p, "main").pixel(4, 4)[0], 1.0, 1e-6, "映像は元のまま");
}
