//! GPU バックエンドの検証: 対応する全エフェクトを、CPU リファレンスの結果と画素ごとに比べる。
//! GPU が使えない環境ではスキップする。

use std::sync::OnceLock;
use znd_core::effect::{effect_def, EvalParams};
use znd_core::{EffectInstance, FrameRate, Time};
use znd_gpu::{GpuBackend, SUPPORTED};
use znd_render::effects::{self, AccelEnv, AccelOp, EffectAccel, FxContext};
use znd_render::{ColorXf, FrameBuf};

fn gpu() -> Option<&'static GpuBackend> {
    static GPU: OnceLock<Option<GpuBackend>> = OnceLock::new();
    GPU.get_or_init(|| match GpuBackend::new() {
        Ok(g) => {
            eprintln!("GPU: {}", g.name());
            Some(g)
        }
        Err(e) => {
            eprintln!("GPU が使えないためスキップ: {e}");
            None
        }
    })
    .as_ref()
}

/// 色の付いたグラデーション、半透明の円、明るい点、透明な余白を持つテスト画像。
fn test_image(w: usize, h: usize) -> FrameBuf {
    let mut b = FrameBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut a = 1.0f32;
            if x < 6 || y < 5 || x >= w - 7 || y >= h - 4 {
                a = 0.0;
            }
            let d = ((x as f32 - w as f32 * 0.65).powi(2) + (y as f32 - h as f32 * 0.4).powi(2)).sqrt();
            if d < h as f32 * 0.22 {
                a *= 0.45;
            }
            let mut c = [0.05 + 0.9 * u, 0.1 + 0.6 * v, 0.8 - 0.7 * u * v];
            // ハイライト（1 を超える値）
            if (x as i32 - w as i32 / 4).abs() < 3 && (y as i32 - h as i32 / 3).abs() < 3 {
                c = [3.0, 2.5, 2.0];
            }
            // 緑バック風の帯
            if y > h * 3 / 4 {
                c = [0.05, 0.8, 0.1];
            }
            b.set_pixel(x, y, [c[0] * a, c[1] * a, c[2] * a, a]);
        }
    }
    b
}

fn cpu(kind: &str, p: &EvalParams, buf: FrameBuf, scale: f32, seconds: f64) -> FrameBuf {
    let cx = ColorXf::SRGB;
    let none_layer = |_: &znd_core::LayerId| None;
    let none_self = |_: Time| None;
    let audio = |_: &znd_core::LayerId, _: usize| Vec::new();
    let eval_at = |_: Time| p.clone();
    let ctx = FxContext {
        time: Time::from_seconds(seconds),
        frame_rate: FrameRate::FPS_30,
        scale,
        cx: &cx,
        fetch_layer: &none_layer,
        fetch_self: &none_self,
        audio: &audio,
        eval_at: &eval_at,
        masks: &[],
        seed: 7,
    };
    effects::apply(kind, p, buf, &ctx)
}

/// (最大差, RMSE)
fn diff(a: &FrameBuf, b: &FrameBuf) -> (f32, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut max = 0.0f32;
    let mut sum = 0.0f64;
    for (x, y) in a.data.iter().zip(&b.data) {
        let d = (x - y).abs();
        max = max.max(d);
        sum += (d as f64).powi(2);
    }
    (max, (sum / a.data.len() as f64).sqrt())
}

fn check(g: &GpuBackend, name: &str, fx: &EffectInstance, scale: f32, seconds: f64) {
    let (w, h) = (397, 261);
    let p = fx.eval(Time::from_seconds(seconds));
    let cx = ColorXf::SRGB;
    let env = AccelEnv { scale, time_seconds: seconds, cx: &cx };
    let op = AccelOp { kind: &fx.kind, params: &p };
    assert!(g.supports(&op, &env, w, h), "{name}: GPU 対応のはず");
    let want = cpu(&fx.kind, &p, test_image(w, h), scale, seconds);
    let got = g.run(&[op], test_image(w, h), &env).unwrap_or_else(|_| panic!("{name}: GPU の実行に失敗"));
    let (max, rmse) = diff(&want, &got);
    // 計算順序と数学関数の実装差だけが残る
    assert!(max < 2e-3 && rmse < 2e-4, "{name}: 最大差 {max}, RMSE {rmse}");
    // 何かしら変化していること（何もしていない実装を通さない）
    let (changed, _) = diff(&test_image(w, h), &got);
    assert!(changed > 1e-3, "{name}: 画像が変化していない");
}

fn fx(kind: &str) -> EffectInstance {
    EffectInstance::new(kind, 397.0, 261.0).unwrap_or_else(|| panic!("unknown effect {kind}"))
}

#[test]
fn g1_every_supported_effect_matches_cpu_reference() {
    let Some(g) = gpu() else { return };
    // 既定値と、分岐を踏む値の組み合わせ
    let cases: Vec<(&str, EffectInstance)> = vec![
        ("gaussian 大（ボックス 3 回）", fx("gaussian_blur").set_f("radius", 24.0)),
        ("gaussian 小（真のカーネル）", fx("gaussian_blur").set_f("radius", 3.0)),
        ("gaussian 水平・端延長", fx("gaussian_blur").set_f("radius", 14.0).set_i("dimensions", 1).set_b("repeat_edge", true)),
        ("gaussian 垂直", fx("gaussian_blur").set_f("radius", 9.0).set_i("dimensions", 2)),
        ("directional", fx("directional_blur").set_f("angle", 30.0).set_f("length", 18.0)),
        ("radial", fx("radial_blur").set_f("amount", 12.0)),
        ("zoom", fx("zoom_blur").set_f("amount", 25.0).set_p("center", [30.0, 20.0])),
        ("glow", fx("glow").set_f("threshold", 0.5).set_f("radius", 20.0).set_f("intensity", 1.5).set_c("color", [1.0, 0.6, 0.3, 1.0])),
        ("bloom", fx("bloom").set_f("threshold", 0.6).set_f("radius", 16.0)),
        ("lens_flare", fx("lens_flare").set_p("center", [25.0, 18.0])),
        ("light_rays", fx("light_rays").set_f("threshold", 0.4).set_f("length", 40.0).set_f("intensity", 2.0)),
        ("drop_shadow", fx("drop_shadow").set_f("distance", 6.0).set_f("softness", 8.0).set_f("angle", 135.0)),
        ("drop_shadow 硬い", fx("drop_shadow").set_f("distance", 4.0).set_f("softness", 0.0)),
        ("levels", fx("levels").set_f("in_black", 0.1).set_f("in_white", 0.8).set_f("gamma", 1.6).set_f("out_black", 0.05)),
        ("hue_saturation", fx("hue_saturation").set_f("hue", 70.0).set_f("saturation", 30.0).set_f("lightness", -20.0)),
        ("hue_saturation 明るく", fx("hue_saturation").set_f("hue", -140.0).set_f("lightness", 25.0)),
        ("color_balance", fx("color_balance").set_f("shadow_r", 40.0).set_f("mid_g", -30.0).set_f("high_b", 50.0)),
        ("tritone", fx("tritone").set_f("blend", 20.0)),
        ("gradient_map", fx("gradient_map").set_c("color_a", [0.1, 0.0, 0.3, 1.0]).set_c("color_b", [1.0, 0.9, 0.2, 1.0])),
        ("shadow_highlight", fx("shadow_highlight").set_f("shadow_amount", 60.0).set_f("highlight_amount", 40.0).set_f("radius", 12.0)),
        ("exposure", fx("exposure").set_f("stops", 0.8).set_f("offset", 0.02)),
        ("brightness_contrast", fx("brightness_contrast").set_f("brightness", 20.0).set_f("contrast", 35.0)),
        ("brightness_contrast 負", fx("brightness_contrast").set_f("contrast", -40.0)),
        ("invert", fx("invert").set_f("blend", 25.0)),
        ("posterize", fx("posterize").set_f("levels", 5.0)),
        ("scanlines", fx("scanlines").set_f("spacing", 6.0).set_f("darkness", 70.0).set_f("speed", 13.0)),
        ("find_edges", fx("find_edges")),
        ("find_edges 反転", fx("find_edges").set_b("invert", true).set_f("blend", 30.0)),
        ("rgb_split", fx("rgb_split").set_f("amount", 5.0).set_f("angle", 20.0)),
        ("rgb_split 放射状", fx("rgb_split").set_f("amount", 9.0).set_b("radial", true)),
        ("wave_warp", fx("wave_warp").set_f("height", 5.0).set_f("width", 22.0).set_f("speed", 0.7)),
        ("wave_warp 三角波", fx("wave_warp").set_f("height", 4.0).set_f("width", 17.0).set_i("wave_type", 2).set_f("direction", 30.0)),
        ("polar 直交→極", fx("polar_coordinates").set_f("interpolation", 100.0)),
        ("polar 極→直交", fx("polar_coordinates").set_f("interpolation", 60.0).set_i("mode", 1)),
        ("mirror", fx("mirror").set_f("angle", 25.0)),
        (
            "motion_tile",
            fx("motion_tile").set_f("tile_width", 40.0).set_f("tile_height", 55.0).set_f("phase", 90.0).set_b("mirror_edges", true),
        ),
        ("chroma_key", fx("chroma_key").set_c("key_color", [0.25, 0.9, 0.35, 1.0]).set_f("despill", 80.0)),
        ("luma_key", fx("luma_key").set_f("threshold", 0.5).set_f("softness", 0.2)),
        ("luma_key 暗い側", fx("luma_key").set_f("threshold", 0.4).set_i("mode", 1)),
        ("matte_choker 縮める", fx("matte_choker").set_f("choke", 3.0).set_f("blur", 2.0)),
        ("matte_choker 広げる", fx("matte_choker").set_f("choke", -3.0)),
    ];
    // 一覧にあるエフェクトがすべて試されていること
    for kind in SUPPORTED {
        assert!(effect_def(kind).is_some(), "{kind} は定義表に無い");
        assert!(cases.iter().any(|(_, f)| f.kind == *kind), "{kind} のテストケースが無い");
    }
    for (name, f) in &cases {
        check(g, name, f, 1.0, 0.4);
    }
    // プレビュー解像度（1/2）でも同じ
    for (name, f) in cases.iter().filter(|(n, _)| ["glow", "drop_shadow", "zoom", "scanlines", "wave_warp"].contains(n)) {
        check(g, &format!("{name} @1/2"), f, 0.5, 0.4);
    }
}

#[test]
fn g2_chain_runs_in_one_round_trip_and_matches_cpu() {
    let Some(g) = gpu() else { return };
    let chain = [
        fx("levels").set_f("gamma", 1.3),
        fx("gaussian_blur").set_f("radius", 6.0),
        fx("glow").set_f("threshold", 0.4).set_f("radius", 14.0),
        fx("hue_saturation").set_f("hue", 40.0),
        fx("drop_shadow").set_f("distance", 5.0).set_f("softness", 6.0),
        fx("rgb_split").set_f("amount", 3.0),
    ];
    let (w, h) = (397, 261);
    let params: Vec<EvalParams> = chain.iter().map(|f| f.eval(Time::ZERO)).collect();
    let mut want = test_image(w, h);
    for (f, p) in chain.iter().zip(&params) {
        want = cpu(&f.kind, p, want, 1.0, 0.0);
    }
    let cx = ColorXf::SRGB;
    let env = AccelEnv { scale: 1.0, time_seconds: 0.0, cx: &cx };
    let ops: Vec<AccelOp> = chain.iter().zip(&params).map(|(f, p)| AccelOp { kind: &f.kind, params: p }).collect();
    let got = g.run(&ops, test_image(w, h), &env).unwrap();
    let (max, rmse) = diff(&want, &got);
    assert!(max < 4e-3 && rmse < 5e-4, "最大差 {max}, RMSE {rmse}");
}

#[test]
fn g3_unsupported_work_falls_back_to_cpu() {
    let Some(g) = gpu() else { return };
    let cx = ColorXf::SRGB;
    let env = AccelEnv { scale: 1.0, time_seconds: 0.0, cx: &cx };
    // GPU 未対応のエフェクト
    let noise = fx("noise");
    let p = noise.eval(Time::ZERO);
    assert!(!g.supports(&AccelOp { kind: "noise", params: &p }, &env, 640, 360));
    // 巨大なぼかしは CPU に回す
    let huge = fx("gaussian_blur").set_f("radius", 1900.0);
    let p = huge.eval(Time::ZERO);
    assert!(!g.supports(&AccelOp { kind: "gaussian_blur", params: &p }, &env, 640, 360));
    // テクスチャの上限を超える画像
    let small = fx("invert");
    let p = small.eval(Time::ZERO);
    assert!(!g.supports(&AccelOp { kind: "invert", params: &p }, &env, 100_000, 8));
    // 小さい画像は転送のほうが高くつくので CPU に回す
    assert!(!g.supports(&AccelOp { kind: "invert", params: &p }, &env, 64, 64));
    assert!(g.supports(&AccelOp { kind: "invert", params: &p }, &env, 640, 360));
}

/// レンダラーに組み込んだ状態で、GPU あり・なしの結果が一致する（調整レイヤー、GPU 未対応エフェクトとの混在を含む）。
#[test]
fn g4_renderer_output_matches_with_and_without_gpu() {
    use znd_core::*;
    use znd_render::{NoFootage, RenderOptions, Renderer};
    let Some(g) = gpu() else { return };
    let d = Time::from_seconds(2.0);
    let (w, h) = (640u32, 360u32);
    let mut a = Layer::new(
        "a",
        "a",
        LayerKind::Solid { width: 400, height: 300, color: [0.9, 0.4, 0.2] },
        Transform::centered(400.0, 300.0, w as f64, h as f64),
        d,
    );
    a.transform.rotation = Property::constant(12.0);
    // GPU → CPU（ノイズは未対応）→ GPU の順に混ざる
    a.effects = vec![
        fx("glow").set_f("threshold", 0.3).set_f("radius", 30.0),
        fx("noise").set_f("amount", 10.0),
        fx("gaussian_blur").set_f("radius", 8.0),
        fx("drop_shadow").set_f("softness", 12.0),
    ];
    let bg = Layer::new(
        "bg",
        "bg",
        LayerKind::Solid { width: w, height: h, color: [0.1, 0.2, 0.4] },
        Transform::centered(w as f64, h as f64, w as f64, h as f64),
        d,
    );
    let mut adj = Layer::new("adj", "adj", LayerKind::Adjustment, Transform::centered(w as f64, h as f64, w as f64, h as f64), d);
    adj.effects = vec![fx("levels").set_f("gamma", 1.4), fx("rgb_split").set_f("amount", 4.0), fx("wave_warp").set_f("height", 6.0)];
    let mut comp = Composition::new("main", "main", w, h, FrameRate::FPS_30, d);
    comp.layers = vec![adj, a, bg];
    let mut p = Project::new("t");
    p.compositions.push(comp);
    let id = CompId::new("main");
    for (downsample, t) in [(1, 0.0), (1, 0.7), (2, 0.7)] {
        let opts = RenderOptions { downsample, ..RenderOptions::EXPORT };
        let t = Time::from_seconds(t);
        let cpu = Renderer::new(&p, &NoFootage).render_frame(&id, t, opts).unwrap();
        let gpu = Renderer::new(&p, &NoFootage).with_accel(Some(g)).render_frame(&id, t, opts).unwrap();
        let (max, rmse) = diff(&cpu, &gpu);
        // 8bit 出力の 1 段（1/255 ≈ 0.004）より小さい
        assert!(max < 3e-3 && rmse < 3e-4, "1/{downsample}: 最大差 {max}, RMSE {rmse}");
    }
}
