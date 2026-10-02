//! M3 受け入れテスト（描画側）: エフェクト、モーションブラー、色深度、カラーマネジメント。

mod common;

use common::*;
use znd_core::effect::EFFECTS;
use znd_core::*;
use znd_render::color::{linear_to_srgb, srgb_to_linear};
use znd_render::{to_rgba8, AudioWindow, FrameBuf, NoFootage, RenderOptions, Renderer};

const N: u32 = 96;

/// エフェクトの素材にするテストパターン: 斜めグラデーションの上に色付きの図形。
fn pattern_layers() -> Vec<Layer> {
    let grad = shape::Gradient {
        radial: false,
        start: Property::constant([-48.0, -48.0]),
        end: Property::constant([48.0, 48.0]),
        stops: vec![(0.0, [0.05, 0.1, 0.3, 1.0]), (1.0, [0.9, 0.8, 0.6, 1.0])],
    };
    let bg = shape(
        "bg",
        vec![
            ShapeItem::rect(96.0, 96.0),
            ShapeItem::Fill {
                color: Property::constant([1.0; 4]),
                opacity: Property::constant(100.0),
                rule: Default::default(),
                gradient: Some(grad),
            },
        ],
        N,
        N,
    );
    let star = shape(
        "star",
        vec![ShapeItem::star(5.0, 30.0, 13.0), ShapeItem::stroke([1.0, 1.0, 1.0, 1.0], 2.0), ShapeItem::fill([1.0, 0.2, 0.3, 1.0])],
        N,
        N,
    );
    let mut dot = shape("dot", vec![ShapeItem::ellipse(18.0, 18.0), ShapeItem::fill([0.1, 1.0, 0.2, 1.0])], N, N);
    dot.transform.position = Property::spatial([74.0, 22.0]);
    vec![dot, star, bg]
}

/// パターンのプリコンポに `effects` を掛けたプロジェクト。`extra` はマップ用などの追加レイヤー。
fn fx_project(effects: Vec<EffectInstance>, extra: Vec<Layer>) -> Project {
    let mut l =
        Layer::new("pat", "pat", LayerKind::Precomp { comp: CompId::new("pattern") }, Transform::centered(96.0, 96.0, 96.0, 96.0), dur());
    l.effects = effects;
    let mut layers = vec![l];
    layers.extend(extra);
    project(vec![comp("c", N, N, layers), comp("pattern", N, N, pattern_layers())])
}

fn apply(effects: Vec<EffectInstance>) -> FrameBuf {
    render_at(&fx_project(effects, vec![]), "c", 0.5, ALPHA)
}

/// マップとして使う、左右のグラデーション（非表示レイヤー）。
fn ramp_map() -> Layer {
    let grad = shape::Gradient {
        radial: false,
        start: Property::constant([-48.0, 0.0]),
        end: Property::constant([48.0, 0.0]),
        stops: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, [1.0, 1.0, 1.0, 1.0])],
    };
    let mut l = shape(
        "map",
        vec![
            ShapeItem::rect(96.0, 96.0),
            ShapeItem::Fill {
                color: Property::constant([1.0; 4]),
                opacity: Property::constant(100.0),
                rule: Default::default(),
                gradient: Some(grad),
            },
        ],
        N,
        N,
    );
    l.visible = false;
    l
}

struct Tone;

impl AudioWindow for Tone {
    fn window(&self, _: &Project, _: &CompId, _: &LayerId, _: Time, len: usize) -> Vec<f32> {
        // 1kHz の正弦波
        (0..len).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin() * 0.8).collect()
    }
}

/// 全エフェクトを、見て分かる程度に効かせた設定で用意する。
fn showcase(kind: &str) -> EffectInstance {
    let e = fx(kind, N, N);
    match kind {
        "gaussian_blur" => e.set_f("radius", 6.0),
        "directional_blur" => e.set_f("angle", 60.0).set_f("length", 20.0),
        "lens_blur" => e.set_f("radius", 6.0).set_f("highlight_gain", 2.0).set_f("highlight_threshold", 0.7),
        "compound_blur" => e.set_layer("map", "map").set_f("max_radius", 12.0),
        "glow" => e.set_f("threshold", 0.7).set_f("radius", 12.0).set_f("intensity", 1.5),
        "bloom" => e.set_f("threshold", 0.6).set_f("radius", 14.0),
        "light_rays" => e.set_f("threshold", 0.6).set_f("length", 50.0),
        "drop_shadow" => e.set_f("distance", 6.0).set_f("softness", 4.0),
        "displacement_map" => e.set_layer("map", "map").set_f("max_h", 14.0).set_f("max_v", 0.0).set_i("channel_h", 4),
        "turbulent_displace" => e.set_f("amount", 12.0).set_f("size", 30.0),
        "wave_warp" => e.set_f("height", 5.0).set_f("width", 30.0),
        "mesh_warp" => {
            let mut o = vec![[0.0, 0.0]; 16];
            o[5] = [14.0, 10.0];
            o[10] = [-12.0, -8.0];
            e.set_curve("offsets", o)
        }
        "liquify" => e.set_f("radius", 22.0).set_curve("strokes", vec![[30.0, 48.0], [56.0, 40.0]]),
        "mirror" => e.set_f("angle", 20.0),
        "motion_tile" => e.set_f("tile_width", 50.0).set_f("tile_height", 50.0).set_b("mirror_edges", true),
        "fractal_noise" => e.set_f("scale", 30.0).set_f("contrast", 180.0).set_f("opacity", 70.0),
        "gradient_ramp" => e.set_c("start_color", [0.9, 0.2, 0.6, 1.0]).set_c("end_color", [0.1, 0.8, 0.9, 1.0]).set_f("opacity", 40.0),
        "cell_pattern" => e.set_f("size", 18.0),
        "checkerboard" => e.set_f("width", 12.0).set_f("height", 12.0).set_f("opacity", 40.0),
        "audio_spectrum" => e
            .set_layer("audio_layer", "pat")
            .set_f("bands", 24.0)
            .set_f("max_height", 60.0)
            .set_p("start", [8.0, 86.0])
            .set_p("end", [88.0, 86.0]),
        "audio_waveform" => e.set_layer("audio_layer", "pat").set_f("max_height", 30.0).set_f("samples", 96.0),
        "echo" => e.set_f("echo_time", -0.1).set_f("count", 3.0),
        "time_displacement" => e.set_layer("map", "map").set_f("max_displacement", 0.4).set_f("steps", 5.0),
        "glitch" => e.set_f("amount", 70.0).set_f("block_size", 9.0),
        "rgb_split" => e.set_f("amount", 4.0),
        "halftone" => e.set_f("dot_size", 6.0),
        "mosaic" => e.set_f("h_blocks", 12.0).set_f("v_blocks", 12.0),
        "noise" => e.set_f("amount", 25.0),
        "film_grain" => e.set_f("amount", 40.0),
        "levels" => e.set_f("in_black", 0.2).set_f("in_white", 0.8).set_f("gamma", 1.4),
        "curves" => e.set_curve("curve", vec![[0.0, 0.0], [0.25, 0.1], [0.75, 0.9], [1.0, 1.0]]),
        "hue_saturation" => e.set_f("hue", 120.0).set_f("saturation", 30.0),
        "color_balance" => e.set_f("shadow_b", 60.0).set_f("high_r", 50.0),
        "shadow_highlight" => e.set_f("shadow_amount", 80.0).set_f("highlight_amount", 40.0),
        "exposure" => e.set_f("stops", 1.0),
        "brightness_contrast" => e.set_f("brightness", 20.0).set_f("contrast", 40.0),
        "chroma_key" => e.set_c("key_color", [0.1, 1.0, 0.2, 1.0]),
        "luma_key" => e.set_f("threshold", 0.6),
        "matte_choker" => e.set_f("choke", 3.0),
        "particles" => e.set_f("rate", 150.0).set_f("life", 1.0).set_f("speed", 70.0).set_f("gravity", 80.0).set_f("size", 4.0),
        _ => e,
    }
}

#[test]
fn f1_every_effect_runs_and_gallery_matches_golden() {
    let mut cells = Vec::new();
    for def in EFFECTS.iter().filter(|d| d.kind != "plugin" && d.kind != "lut" && d.category != "エクスプレッション制御") {
        let mut l = Layer::new(
            "pat",
            "pat",
            LayerKind::Precomp { comp: CompId::new("pattern") },
            Transform::centered(96.0, 96.0, 96.0, 96.0),
            dur(),
        );
        let mut effects = vec![showcase(def.kind)];
        // 線・落書きはマスクパスを使う
        if matches!(def.kind, "stroke" | "scribble") {
            let mut m = Mask::new(BezPath::ellipse([48.0, 48.0], [32.0, 24.0]));
            m.mode = MaskMode::None;
            l.masks = vec![m];
        }
        // 時間系は動く素材でないと効果が見えないので、位置をアニメートする
        if def.category == "時間" {
            l.transform.position = Property::spatial([48.0, 48.0])
                .with_keys(vec![Keyframe::linear(Time::ZERO, [20.0, 48.0]), Keyframe::linear(Time::from_seconds(1.0), [76.0, 48.0])]);
            // 位置アニメはレイヤーの外なので、エコーが見えるようコンポごとプリコンポにする
            let moving = comp(
                "moving",
                N,
                N,
                vec![{
                    let mut m = l.clone();
                    m.effects.clear();
                    m
                }],
            );
            let mut outer = Layer::new(
                "pat",
                "pat",
                LayerKind::Precomp { comp: CompId::new("moving") },
                Transform::centered(96.0, 96.0, 96.0, 96.0),
                dur(),
            );
            outer.effects = std::mem::take(&mut effects);
            let p = project(vec![comp("c", N, N, vec![outer, ramp_map()]), moving, comp("pattern", N, N, pattern_layers())]);
            let out = Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), Time::from_seconds(0.5), ALPHA).unwrap();
            assert!(all_finite(&out), "{}", def.kind);
            cells.push(out);
            continue;
        }
        l.effects = effects;
        let p = project(vec![comp("c", N, N, vec![l, ramp_map()]), comp("pattern", N, N, pattern_layers())]);
        let out = Renderer::new(&p, &NoFootage).with_audio(&Tone).render_frame(&CompId::new("c"), Time::from_seconds(0.5), ALPHA).unwrap();
        assert_eq!((out.width, out.height), (96, 96), "{}", def.kind);
        assert!(all_finite(&out), "{}: 非有限値", def.kind);
        assert!(out.data.chunks_exact(4).all(|p| p[3] >= -1e-4 && p[3] <= 1.0 + 1e-4), "{}: アルファが範囲外", def.kind);
        cells.push(out);
    }
    // ノイズ等を含むが、乱数は画素位置・フレーム・レイヤー ID から決まるので毎回同じ絵になる
    golden("effects", &atlas(&cells, 10));
}

#[test]
fn f2_blur_conserves_energy_and_is_resolution_independent() {
    // 中央の小さな白い正方形をぼかす
    let mut l = solid("s", 20, 20, WHITE, 120, 120);
    let base = render(&project(vec![comp("c", 120, 120, vec![l.clone()])]), "c");
    // エフェクトはレイヤーの範囲内で掛かるので、余白のあるプリコンポ経由でぼかす
    let mut pre =
        Layer::new("pre", "pre", LayerKind::Precomp { comp: CompId::new("inner") }, Transform::centered(120.0, 120.0, 120.0, 120.0), dur());
    pre.effects = vec![fx("gaussian_blur", 120, 120).set_f("radius", 16.0)];
    l.name = "s".into();
    let p = project(vec![comp("c", 120, 120, vec![pre]), comp("inner", 120, 120, vec![l])]);
    let blurred = render(&p, "c");
    assert!((alpha_sum(&blurred) - alpha_sum(&base)).abs() / alpha_sum(&base) < 0.01);
    assert!(blurred.pixel(60, 60)[3] < 0.9 && blurred.pixel(60, 40)[3] > 0.02);
    // 1/2 解像度でも同じ見た目（総量は面積比の 1/4）
    let half = render_at(&p, "c", 0.0, RenderOptions { downsample: 2, ..ALPHA });
    assert!((alpha_sum(&half) * 4.0 - alpha_sum(&blurred)).abs() / alpha_sum(&blurred) < 0.02);
    assert!((half.pixel(30, 30)[3] - blurred.pixel(60, 60)[3]).abs() < 0.03);
}

fn max_diff(a: &FrameBuf, b: &FrameBuf) -> f32 {
    a.data.iter().zip(&b.data).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

#[test]
fn f3_color_effects_identities_and_exact_values() {
    let base = apply(vec![]);
    // 既定値のレベル・カーブ・色相彩度・カラーバランス・露光は何も変えない
    for kind in ["levels", "curves", "hue_saturation", "color_balance", "exposure", "brightness_contrast"] {
        let d = max_diff(&apply(vec![fx(kind, N, N)]), &base);
        assert!(d < 2e-3, "{kind}: {d}");
    }
    // 反転 2 回、色相 360° で元に戻る
    assert!(max_diff(&apply(vec![fx("invert", N, N), fx("invert", N, N)]), &base) < 2e-3);
    assert!(max_diff(&apply(vec![fx("hue_saturation", N, N).set_f("hue", 360.0)]), &base) < 2e-3);
    // 露光 +1 段は光量 2 倍（リニア）
    let bright = apply(vec![fx("exposure", N, N).set_f("stops", 1.0)]);
    let (a, b) = (base.pixel(10, 10), bright.pixel(10, 10));
    assert!((b[0] / a[0] - 2.0).abs() < 1e-4);
    // 彩度 -100 はグレー
    let gray = apply(vec![fx("hue_saturation", N, N).set_f("saturation", -100.0)]);
    let g = gray.pixel(48, 48);
    assert!((g[0] - g[1]).abs() < 1e-3 && (g[1] - g[2]).abs() < 1e-3);
    // ポスタリゼーション 2 段: 表示値は 0 か 1 だけ
    let post = apply(vec![fx("posterize", N, N).set_f("levels", 2.0)]);
    assert!(post.data.chunks_exact(4).all(|p| p[..3].iter().all(|v| v.abs() < 1e-4 || (v - 1.0).abs() < 1e-4)));
    // レベル: 入力黒 0.5 → 表示値 0.5 以下は黒に落ちる
    let lv = apply(vec![fx("levels", N, N).set_f("in_black", 0.5)]);
    let (src, dst) = (base.pixel(5, 5), lv.pixel(5, 5));
    assert!(linear_to_srgb(src[0]) < 0.5 && dst[0].abs() < 1e-5);
    // トーンカーブ: 0.5 → 0.25 に落とすカーブ
    let cv = apply(vec![fx("curves", N, N).set_curve("curve", vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]])]);
    let s = linear_to_srgb(base.pixel(48, 5)[1]);
    let expect = if s <= 0.5 { s * 0.5 } else { 0.25 + (s - 0.5) * 1.5 };
    assert!((linear_to_srgb(cv.pixel(48, 5)[1]) - expect).abs() < 2e-3);
}

#[test]
fn f4_lut_file_identity_and_tint() {
    let dir = std::env::temp_dir().join(format!("znd-lut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, f: &dyn Fn([f32; 3]) -> [f32; 3]| {
        let n = 9;
        let mut s = format!("LUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = f([r as f32 / 8.0, g as f32 / 8.0, b as f32 / 8.0]);
                    s += &format!("{} {} {}\n", c[0], c[1], c[2]);
                }
            }
        }
        let path = dir.join(name);
        std::fs::write(&path, s).unwrap();
        path.display().to_string()
    };
    let base = apply(vec![]);
    let id = write("id.cube", &|c| c);
    assert!(max_diff(&apply(vec![fx("lut", N, N).set_s("path", &id)]), &base) < 2e-3);
    // 赤を 0 にする LUT
    let nored = write("nored.cube", &|c| [0.0, c[1], c[2]]);
    let out = apply(vec![fx("lut", N, N).set_s("path", &nored)]);
    assert!(out.data.chunks_exact(4).all(|p| p[0].abs() < 1e-4));
    // 適用量 50% は中間
    let half = apply(vec![fx("lut", N, N).set_s("path", &nored).set_f("intensity", 50.0)]);
    let (a, h) = (linear_to_srgb(base.pixel(48, 48)[0]), linear_to_srgb(half.pixel(48, 48)[0]));
    assert!((h - a / 2.0).abs() < 2e-3);
    // 存在しないファイルは何もしない（クラッシュしない）
    assert!(max_diff(&apply(vec![fx("lut", N, N).set_s("path", "/nonexistent.cube")]), &base) < 1e-6);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn f5_keying() {
    // 緑の円がキーで抜け、赤い星は残る。スピル除去で緑かぶりが消える
    let out = apply(vec![fx("chroma_key", N, N).set_c("key_color", [0.1, 1.0, 0.2, 1.0])]);
    assert!(out.pixel(74, 22)[3] < 0.02, "緑の円が残っている: {:?}", out.pixel(74, 22));
    assert!(out.pixel(48, 48)[3] > 0.98, "星が抜けている: {:?}", out.pixel(48, 48));
    assert!(out.pixel(10, 86)[3] > 0.98);
    // ルミナンスキー: 明るい右下が抜け、暗い左上が残る
    let lk = apply(vec![fx("luma_key", N, N).set_f("threshold", 0.5).set_f("softness", 0.05)]);
    assert!(lk.pixel(4, 4)[3] > 0.95 && lk.pixel(92, 92)[3] < 0.05);
}

#[test]
fn f6_fractal_noise_loops_and_displacement_moves_pixels() {
    let noise = |evolution: f64| {
        apply(vec![fx("fractal_noise", N, N)
            .set_f("scale", 25.0)
            .set_b("loop_evolution", true)
            .set_f("cycle", 2.0)
            .set_f("evolution", evolution)])
    };
    // サイクル 2 周: 展開 0° と 720° が同じ絵になる
    assert!(max_diff(&noise(0.0), &noise(720.0)) < 1e-4);
    assert!(max_diff(&noise(0.0), &noise(200.0)) > 0.1);
    // ノイズは全面不透明で、平均は中間グレー付近
    let n = noise(0.0);
    let mean: f32 = n.data.chunks_exact(4).map(|p| p[0]).sum::<f32>() / (96.0 * 96.0);
    assert!(n.data.chunks_exact(4).all(|p| p[3] == 1.0) && mean > 0.3 && mean < 0.7, "{mean}");

    // ディスプレイスメント: マップが明るい右側は +方向へ、暗い左側は -方向へずれる
    let p = fx_project(
        vec![fx("displacement_map", N, N).set_layer("map", "map").set_f("max_h", 10.0).set_f("max_v", 0.0).set_i("channel_h", 4)],
        vec![ramp_map()],
    );
    let out = render_at(&p, "c", 0.0, ALPHA);
    let base = apply(vec![]);
    // 緑の円（中心 x=74。マップはリニアで 0.77 ＝ 表示値 0.89 → 約 +8px）
    let cx = |b: &FrameBuf| {
        let (mut s, mut n) = (0.0, 0.0);
        for y in 0..96 {
            for x in 0..96 {
                let p = b.pixel(x, y);
                if p[1] > 0.8 && p[0] < 0.2 {
                    s += x as f64;
                    n += 1.0;
                }
            }
        }
        s / n
    };
    let shift = cx(&out) - cx(&base);
    assert!(shift > 5.0 && shift < 11.0, "shift {shift}");
}

#[test]
fn f7_echo_and_time_effects_use_other_times() {
    // 右へ動く白い正方形（プリコンポ内）にエコー → 過去の位置に薄い残像が出る
    let mut sq = solid("sq", 10, 10, WHITE, 100, 20);
    sq.transform.position = Property::spatial([0.0, 10.0])
        .with_keys(vec![Keyframe::linear(Time::ZERO, [10.0, 10.0]), Keyframe::linear(Time::from_seconds(1.0), [90.0, 10.0])]);
    let mut pre =
        Layer::new("pre", "pre", LayerKind::Precomp { comp: CompId::new("inner") }, Transform::centered(100.0, 20.0, 100.0, 20.0), dur());
    pre.effects = vec![fx("echo", 100, 20).set_f("echo_time", -0.25).set_f("count", 2.0).set_f("decay", 0.5).set_i("operator", 1)];
    let p = project(vec![comp("c", 100, 20, vec![pre]), comp("inner", 100, 20, vec![sq])]);
    let out = render_at(&p, "c", 0.5, ALPHA);
    // 現在位置 x=50、0.25 秒前 x=30（強度 0.5）、0.5 秒前 x=10（強度 0.25）
    assert_px(&out, 50, 10, [1.0; 4], 1e-4);
    assert_px(&out, 30, 10, [0.5; 4], 1e-4);
    assert_px(&out, 10, 10, [0.25; 4], 1e-4);
    assert_px(&out, 70, 10, [0.0; 4], 1e-4);
}

#[test]
fn b1_motion_blur_profile_matches_shutter() {
    // 1 フレームで 30px 右へ動く 10px 幅の正方形。シャッター 180°（半フレーム露光 = 15px の移動）
    let fr = FrameRate::FPS_30;
    let mut sq = solid("sq", 10, 10, WHITE, 200, 20);
    sq.transform.position = Property::spatial([0.0, 10.0])
        .with_keys(vec![Keyframe::linear(Time::ZERO, [20.0, 10.0]), Keyframe::linear(fr.frame_to_time(4), [140.0, 10.0])]);
    sq.motion_blur = true;
    let mut c = comp("c", 200, 20, vec![sq]);
    c.motion_blur = MotionBlur { enabled: true, shutter_angle: 180.0, shutter_phase: -90.0, samples: 64 };
    let p = project(vec![c]);
    let out = Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), fr.frame_to_time(2), ALPHA).unwrap();
    // フレーム 2 の中心 x=80。露光中に中心は 72.5..87.5 を動く
    // → 被覆率の理論値は台形: 67.5 から立ち上がり 77.5..82.5 で最大 10/15、92.5 で 0
    let ideal = |x: f64| -> f64 {
        let d = (x - 80.0).abs();
        if d <= 2.5 {
            10.0 / 15.0
        } else if d >= 12.5 {
            0.0
        } else {
            (12.5 - d) / 15.0
        }
    };
    let mut err = 0.0;
    for x in 60..100 {
        let got = out.pixel(x, 10)[3] as f64;
        err += (got - ideal(x as f64 + 0.5)).powi(2);
    }
    let rmse = (err / 40.0).sqrt();
    assert!(rmse < 0.01, "rmse {rmse}");
    // 総量（面積）は保存される
    assert!((alpha_sum(&out) - 100.0).abs() < 0.5);
    // モーションブラーを切ればくっきり
    let mut p2 = p.clone();
    p2.compositions[0].motion_blur.enabled = false;
    let sharp = Renderer::new(&p2, &NoFootage).render_frame(&CompId::new("c"), fr.frame_to_time(2), ALPHA).unwrap();
    assert_px(&sharp, 80, 10, [1.0; 4], 1e-5);
    assert_px(&sharp, 70, 10, [0.0; 4], 1e-5);
}

#[test]
fn q1_bit_depth_modes_clamp_and_quantize() {
    // 加算で 1 を超える合成の上に乗算 0.5: 32bpc は 1 超を保持、8bpc は加算の時点で 1 に切られる
    let mut mul = solid("mul", 8, 8, [0.5, 0.5, 0.5], 8, 8);
    mul.blend_mode = BlendMode::Multiply;
    let mut add = solid("add", 8, 8, WHITE, 8, 8);
    add.blend_mode = BlendMode::Add;
    let mut p = project(vec![comp("c", 8, 8, vec![mul, add, solid("bg", 8, 8, WHITE, 8, 8)])]);
    let half = srgb_to_linear(0.5);
    assert_px(&render(&p, "c"), 4, 4, [2.0 * half, 2.0 * half, 2.0 * half, 1.0], 1e-4);
    p.settings.bit_depth = BitDepth::B8;
    let out = render(&p, "c");
    assert_px(&out, 4, 4, [half, half, half, 1.0], 2e-3);
    // 8bpc の結果は 8bit の表示値にちょうど乗っている
    let d = linear_to_srgb(out.pixel(4, 4)[0]) * 255.0;
    assert!((d - d.round()).abs() < 1e-3, "{d}");
}

#[test]
fn q2_working_space_roundtrips_to_srgb_output() {
    // 作業色空間を P3 / ACEScg にしても、sRGB 出力なら sRGB の色指定どおりに表示される
    for space in [ColorSpace::Srgb, ColorSpace::DisplayP3, ColorSpace::AcesCg] {
        let mut p = project(vec![comp("c", 8, 8, vec![solid("s", 8, 8, [0.8, 0.3, 0.1], 8, 8)])]);
        p.settings.working_space = space;
        let r = Renderer::new(&p, &NoFootage);
        let buf = r.render_frame(&CompId::new("c"), Time::ZERO, ALPHA).unwrap();
        let px = znd_render::output::encode_rgba8(&buf, None, &r.output_xf());
        assert!(
            (px[0] as i32 - 204).abs() <= 1 && (px[1] as i32 - 77).abs() <= 1 && (px[2] as i32 - 26).abs() <= 1,
            "{space:?}: {:?}",
            &px[..4]
        );
    }
    // sRGB 作業・P3 出力では、同じ色が P3 の符号値（彩度が低い数値）になる
    let mut p = project(vec![comp("c", 8, 8, vec![solid("s", 8, 8, RED, 8, 8)])]);
    p.settings.output_space = ColorSpace::DisplayP3;
    let r = Renderer::new(&p, &NoFootage);
    let px = znd_render::output::encode_rgba8(&r.render_frame(&CompId::new("c"), Time::ZERO, ALPHA).unwrap(), None, &r.output_xf());
    assert!(px[0] < 240 && px[1] > 40, "{:?}", &px[..4]);
    let _ = to_rgba8;
}
