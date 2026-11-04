//! M3 受け入れテスト: エクスプレッション互換（X1）、エラー表示（X2）、自動化スクリプト。

use znd_core::*;
use znd_render::{NoFootage, RenderOptions, Renderer};
use znd_script::{run_script, ExprEngine};

fn s(v: f64) -> Time {
    Time::from_seconds(v)
}

fn expr(src: &str) -> Option<Expression> {
    Some(Expression { source: src.into(), enabled: true })
}

fn solid(id: &str) -> Layer {
    Layer::new(
        id,
        id,
        LayerKind::Solid { width: 100, height: 100, color: [1.0, 1.0, 1.0] },
        Transform::centered(100.0, 100.0, 400.0, 300.0),
        s(10.0),
    )
}

fn project(layers: Vec<Layer>) -> Project {
    let mut p = Project::new("x");
    let mut c = Composition { layers, ..Composition::new("c", "メイン", 400, 300, FrameRate::FPS_30, s(10.0)) };
    c.markers = vec![
        Marker { time: s(1.0), duration: Time::ZERO, comment: "サビ".into() },
        Marker { time: s(3.0), duration: Time::ZERO, comment: "落ち".into() },
    ];
    p.compositions.push(c);
    p
}

fn eval(p: &Project, layer: &str, path: &str, t: f64) -> Vec<f64> {
    ExprEngine::new().eval_property(p, &CompId::new("c"), &LayerId::new(layer), path, s(t)).unwrap_or_else(|e| panic!("{e}"))
}

/// 回転プロパティに式を書いて、時刻 t の値を得る。
fn rot(src: &str, t: f64) -> f64 {
    let mut l = solid("a");
    l.transform.rotation = Property::constant(5.0);
    l.transform.rotation.expression = expr(src);
    eval(&project(vec![l]), "a", "transform.rotation", t)[0]
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn x1_basics_time_value_index_and_math() {
    assert!(close(rot("time * 90", 2.0), 180.0));
    assert!(close(rot("value + 10", 0.0), 15.0));
    assert!(close(rot("index * 10", 0.0), 10.0));
    assert!(close(rot("Math.sin(Math.PI / 2) * 3", 0.0), 3.0));
    // 最後の式の値が結果になる（複数行・変数）
    assert!(close(rot("var a = 2;\nvar b = a * 3;\nb + time", 1.0), 7.0));
    assert!(close(rot("thisComp.width + thisComp.height + thisComp.numLayers", 0.0), 701.0));
    assert!(close(rot("thisComp.frameDuration * 30", 0.0), 1.0));
    assert!(close(rot("thisLayer.width + inPoint + outPoint", 0.0), 110.0));
    assert!(close(rot("timeToFrames(time) + framesToTime(15)", 1.0), 30.5));
    assert!(close(rot("degreesToRadians(180) / Math.PI + radiansToDegrees(Math.PI)", 0.0), 181.0));
}

#[test]
fn x1_interpolation_helpers() {
    assert!(close(rot("linear(time, 0, 2, 0, 100)", 0.5), 25.0));
    assert!(close(rot("linear(time, 0, 2, 0, 100)", 5.0), 100.0));
    assert!(close(rot("linear(time, 1, 3, 100, 0)", 0.0), 100.0));
    assert!(close(rot("linear(0.25, 0, 100)", 0.0), 25.0));
    assert!(close(rot("ease(time, 0, 2, 0, 100)", 1.0), 50.0));
    assert!(close(rot("ease(time, 0, 2, 0, 100)", 0.5), 100.0 * (3.0 * 0.0625 - 2.0 * 0.015625)));
    assert!(close(rot("easeIn(time, 0, 2, 0, 100)", 1.0), 25.0));
    assert!(close(rot("easeOut(time, 0, 2, 0, 100)", 1.0), 75.0));
    assert!(close(rot("clamp(time * 100, 20, 50)", 1.0), 50.0));
    assert!(close(rot("length([3, 4]) + length([0, 0], [6, 8])", 0.0), 15.0));
    assert!(close(rot("dot([1, 2, 3], [4, 5, 6]) + cross([1, 0, 0], [0, 1, 0])[2]", 0.0), 33.0));
    assert!(close(rot("var v = normalize([3, 4]); v[0] * 10 + v[1] * 10", 0.0), 14.0));
    // 配列の補間
    let mut l = solid("a");
    l.transform.position.expression = expr("linear(time, 0, 1, [0, 0], [100, 50])");
    assert_eq!(eval(&project(vec![l]), "a", "transform.position", 0.5), vec![50.0, 25.0]);
}

#[test]
fn x1_vector_math_and_value_arrays() {
    let mut l = solid("a");
    l.transform.position = Property::spatial([100.0, 50.0]);
    l.transform.position.expression = expr("add(value, [10, -5])");
    assert_eq!(eval(&project(vec![l.clone()]), "a", "transform.position", 0.0), vec![110.0, 45.0]);
    l.transform.position.expression = expr("[value[0] * 2, value[1] + time]");
    assert_eq!(eval(&project(vec![l.clone()]), "a", "transform.position", 3.0), vec![200.0, 53.0]);
    l.transform.position.expression = expr("mul(sub(value, [50, 0]), 2)");
    assert_eq!(eval(&project(vec![l.clone()]), "a", "transform.position", 0.0), vec![100.0, 100.0]);
    l.transform.position.expression = expr("div(value, 2)");
    assert_eq!(eval(&project(vec![l]), "a", "transform.position", 0.0), vec![50.0, 25.0]);
}

#[test]
fn x1_layer_references_and_value_at_time() {
    let mut leader = solid("leader");
    leader.name = "リーダー".into();
    leader.transform.position =
        Property::spatial([0.0, 0.0]).with_keys(vec![Keyframe::linear(s(0.0), [0.0, 100.0]), Keyframe::linear(s(4.0), [400.0, 100.0])]);
    leader.transform.rotation = Property::constant(30.0);
    let mut follower = solid("follower");
    // 0.5 秒遅れて追従する
    follower.transform.position.expression = expr("thisComp.layer(\"リーダー\").transform.position.valueAtTime(time - 0.5)");
    follower.transform.rotation.expression = expr("thisComp.layer(index - 1).transform.rotation + 10");
    follower.transform.opacity.expression = expr("thisComp.layer(1).position[0] / 4");
    follower.parent = Some(LayerId::new("leader"));
    follower.transform.scale.expression = expr("hasParent ? [parent.rotation.value, parent.index] : [0, 0]");
    let p = project(vec![leader, follower]);
    assert_eq!(eval(&p, "follower", "transform.position", 2.0), vec![150.0, 100.0]);
    assert_eq!(eval(&p, "follower", "transform.rotation", 0.0), vec![40.0]);
    assert_eq!(eval(&p, "follower", "transform.opacity", 1.0), vec![25.0]);
    assert_eq!(eval(&p, "follower", "transform.scale", 0.0), vec![30.0, 1.0]);
    // 速度
    let mut l = solid("v");
    l.transform.rotation.expression = expr("thisComp.layer(\"リーダー\").position.speedAtTime(1)");
    let mut p2 = p.clone();
    p2.compositions[0].layers.push(l);
    assert!((eval(&p2, "v", "transform.rotation", 0.0)[0] - 100.0).abs() < 1e-3);
}

#[test]
fn x1_expressions_chain_through_other_expressions() {
    // B は A のエクスプレッション適用後の値を見る
    let mut a = solid("a");
    a.transform.rotation.expression = expr("time * 10");
    let mut b = solid("b");
    b.transform.rotation.expression = expr("thisComp.layer(\"a\").rotation * 2");
    let p = project(vec![a, b]);
    assert_eq!(eval(&p, "b", "transform.rotation", 3.0), vec![60.0]);
    // 別時刻の値も、参照先のエクスプレッションをその時刻で評価する
    let mut c = solid("c");
    c.transform.rotation.expression = expr("thisComp.layer(\"a\").rotation.valueAtTime(time - 1)");
    let mut p2 = p.clone();
    p2.compositions[0].layers.push(c);
    assert_eq!(eval(&p2, "c", "transform.rotation", 3.0), vec![20.0]);
}

fn looping(kind: &str, t: f64) -> f64 {
    let mut l = solid("a");
    l.transform.rotation = Property::constant(0.0).with_keys(vec![
        Keyframe::linear(s(1.0), 0.0),
        Keyframe::linear(s(2.0), 100.0),
        Keyframe::linear(s(3.0), 40.0),
    ]);
    l.transform.rotation.expression = expr(kind);
    eval(&project(vec![l]), "a", "transform.rotation", t)[0]
}

#[test]
fn x1_loops() {
    // キーは 1s:0, 2s:100, 3s:40（周期 2 秒）
    assert!(close(looping("loopOut()", 2.5), 70.0), "キーの範囲内はそのまま");
    assert!(close(looping("loopOut()", 3.5), 50.0), "cycle: 3.5 → 1.5");
    assert!(close(looping("loopOut('cycle')", 6.25), 85.0), "6.25 → 2.25");
    assert!(close(looping("loopOut('pingpong')", 3.5), 70.0), "pingpong: 3.5 → 2.5");
    assert!(close(looping("loopOut('pingpong')", 5.5), 50.0), "2 周目は順方向: 5.5 → 1.5");
    assert!(close(looping("loopOut('offset')", 3.5), 90.0), "offset: 50 + (40 - 0)");
    assert!(close(looping("loopOut('offset')", 5.5), 130.0));
    // continue: 最後の区間の速度（-60/秒）で延長
    assert!((looping("loopOut('continue')", 4.0) - (40.0 - 60.0)).abs() < 0.1);
    // 最後の 1 区間だけを繰り返す
    assert!(close(looping("loopOut('cycle', 1)", 3.5), 70.0));
    // loopIn は最初のキーより前で繰り返す
    assert!(close(looping("loopIn()", 0.5), 70.0), "0.5 → 2.5");
    assert!(close(looping("loopIn('pingpong')", 0.5), 50.0), "0.5 → 1.5");
    assert!(close(looping("numKeys * 10 + key(2).time + key(3).value", 0.0), 72.0));
    assert!(close(looping("nearestKey(time).index", 2.4), 2.0));
}

#[test]
fn x1_wiggle_is_deterministic_bounded_and_centered() {
    let wig = |t: f64| rot("wiggle(3, 20)", t);
    assert_eq!(wig(1.234), wig(1.234));
    let samples: Vec<f64> = (0..600).map(|i| wig(i as f64 / 30.0)).collect();
    let (min, max) = samples.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    assert!(min >= 5.0 - 20.0 - 1e-9 && max <= 5.0 + 20.0 + 1e-9, "範囲 {min}..{max}");
    assert!(max - min > 15.0, "十分に揺れていること: {min}..{max}");
    assert!((mean - 5.0).abs() < 3.0, "平均 {mean}");
    // 周波数: 3 回/秒 → 20 秒で、平均値を横切る回数は 60 回前後
    let crossings = samples.windows(2).filter(|w| (w[0] - 5.0) * (w[1] - 5.0) < 0.0).count();
    assert!((30..=90).contains(&crossings), "交差 {crossings} 回");
    // 連続している（1 フレームで大きく飛ばない）
    assert!(samples.windows(2).all(|w| (w[0] - w[1]).abs() < 8.0));
    // 多次元は次元ごとに違う動き。レイヤーが違えば違う動き
    let mut a = solid("a");
    a.transform.position.expression = expr("wiggle(2, 50)");
    let mut b = solid("b");
    b.transform.position.expression = expr("wiggle(2, 50)");
    let p = project(vec![a, b]);
    let (va, vb) = (eval(&p, "a", "transform.position", 0.7), eval(&p, "b", "transform.position", 0.7));
    assert!((va[0] - 200.0 - (va[1] - 150.0)).abs() > 1e-3);
    assert!((va[0] - vb[0]).abs() > 1e-3);
}

#[test]
fn x1_random_seed_and_posterize_time() {
    // 既定の random はフレームごとに変わる
    assert_ne!(rot("random(0, 100)", 1.0), rot("random(0, 100)", 1.1));
    assert_eq!(rot("random(0, 100)", 1.0), rot("random(0, 100)", 1.0));
    // timeless なら時刻に依らず一定。種を変えると値が変わる
    assert_eq!(rot("seedRandom(3, true); random(0, 100)", 1.0), rot("seedRandom(3, true); random(0, 100)", 7.0));
    assert_ne!(rot("seedRandom(3, true); random(0, 100)", 1.0), rot("seedRandom(4, true); random(0, 100)", 1.0));
    let v = rot("seedRandom(1, true); random(10, 20)", 0.0);
    assert!((10.0..=20.0).contains(&v));
    let g = rot("seedRandom(1, true); gaussRandom()", 0.0);
    assert!((0.0..=1.0).contains(&g));
    assert!(close(rot("random([10, 20]).length", 0.0), 2.0));
    // posterizeTime: 2 fps に落とすと 0.5 秒ごとに階段状になる
    assert!(close(rot("posterizeTime(2); time * 10", 0.7), 5.0));
    assert!(close(rot("posterizeTime(2); time * 10", 0.99), 5.0));
    assert!(close(rot("posterizeTime(2); time * 10", 1.0), 10.0));
    // posterizeTime 後の value はその時刻の値
    let mut l = solid("a");
    l.transform.rotation = Property::constant(0.0).with_keys(vec![Keyframe::linear(s(0.0), 0.0), Keyframe::linear(s(10.0), 100.0)]);
    l.transform.rotation.expression = expr("posterizeTime(1); value");
    assert!(close(eval(&project(vec![l]), "a", "transform.rotation", 3.9)[0], 30.0));
}

#[test]
fn x1_effect_controls_markers_source_rect_and_to_comp() {
    let mut ctl = Layer::new("ctl", "制御", LayerKind::Null, Transform::centered(0.0, 0.0, 400.0, 300.0), s(10.0));
    let mut slider = EffectInstance::new("slider_control", 0.0, 0.0).unwrap().set_f("slider", 42.0);
    slider.name = "強さ".into();
    let target = EffectInstance::new("layer_control", 0.0, 0.0).unwrap().set_layer("layer", "box");
    ctl.effects = vec![slider, target];
    let mut shape = Layer::new(
        "box",
        "箱",
        LayerKind::Shape { items: vec![ShapeItem::rect(60.0, 40.0), ShapeItem::fill([1.0; 4])] },
        Transform::centered(0.0, 0.0, 400.0, 300.0),
        s(10.0),
    );
    shape.transform.rotation = Property::constant(90.0);
    let mut a = solid("a");
    a.transform.rotation.expression = expr("thisComp.layer(\"制御\").effect(\"強さ\")(\"スライダー\") * 2");
    a.transform.opacity.expression = expr("thisComp.layer(\"制御\").effect(1)(\"slider\") + thisComp.layer(\"制御\").effect(\"強さ\")(1)");
    a.transform.scale.expression = expr("var r = thisComp.layer(\"箱\").sourceRectAtTime(time); [r.width, r.height]");
    a.transform.anchor.expression = expr("var r = thisComp.layer(\"箱\").sourceRectAtTime(); [r.left, r.top]");
    a.transform.position.expression = expr("thisComp.layer(\"箱\").toComp([10, 0])");
    a.transform.rotation_x.expression = expr("thisComp.marker.nearestKey(time).time * 10 + thisComp.marker.numKeys");
    a.transform.rotation_y.expression =
        expr("thisComp.marker.key(\"落ち\").time + thisComp.layer(\"制御\").effect(\"レイヤー制御\")(\"レイヤー\").index");
    let p = project(vec![ctl, shape, a]);
    assert_eq!(eval(&p, "a", "transform.rotation", 0.0), vec![84.0]);
    assert_eq!(eval(&p, "a", "transform.opacity", 0.0), vec![84.0]);
    let size = eval(&p, "a", "transform.scale", 0.0);
    assert!((size[0] - 60.0).abs() < 0.5 && (size[1] - 40.0).abs() < 0.5, "{size:?}");
    let origin = eval(&p, "a", "transform.anchor", 0.0);
    assert!((origin[0] + 30.0).abs() < 0.5 && (origin[1] + 20.0).abs() < 0.5, "{origin:?}");
    // 箱は (200,150) にあり 90° 回転。レイヤー座標 (10, 0) はコンポ座標 (200, 160)
    let pt = eval(&p, "a", "transform.position", 0.0);
    assert!((pt[0] - 200.0).abs() < 1e-6 && (pt[1] - 160.0).abs() < 1e-6, "{pt:?}");
    assert_eq!(eval(&p, "a", "transform.rotation_x", 1.4), vec![12.0]);
    assert_eq!(eval(&p, "a", "transform.rotation_x", 2.6), vec![32.0]);
    assert_eq!(eval(&p, "a", "transform.rotation_y", 0.0), vec![5.0]);
}

fn error_of(src: &str) -> znd_script::ExprError {
    let mut l = solid("a");
    l.name = "タイトル".into();
    l.transform.rotation.expression = expr(src);
    ExprEngine::new().eval_property(&project(vec![l]), &CompId::new("c"), &LayerId::new("a"), "transform.rotation", Time::ZERO).unwrap_err()
}

#[test]
fn x2_errors_report_layer_property_and_line() {
    let e = error_of("var a = 1;\nvar b = 2;\nfoo.bar + a");
    assert_eq!(e.line, Some(3), "{e}");
    assert!(e.message.contains("foo") && e.layer == "タイトル" && e.path == "transform.rotation", "{e}");
    assert!(e.to_string().contains("3 行目"), "{e}");
    // 構文エラー
    let e = error_of("var a = 1;\nvar b = ;\na");
    assert_eq!(e.line, Some(2), "{e}");
    // 自分で投げた例外
    let e = error_of("\n\n\nthrow new Error('だめ')");
    assert_eq!(e.line, Some(4), "{e}");
    assert!(e.message.contains("だめ"));
    // 存在しないレイヤー・範囲外のキー
    let e = error_of("var x = 1;\nthisComp.layer(\"無い\").position[0]");
    assert!(e.message.contains("無い") && e.line == Some(2), "ライブラリ内で投げた例外も、式の中の行で報告する: {e}");
    assert!(error_of("thisComp.layer(9).rotation").message.contains("範囲外"));
    assert!(error_of("key(1).time").message.contains("範囲外"));
    // 数値以外・次元不足
    assert!(error_of("'文字列'").message.contains("数値"));
    assert!(error_of("[1, 2] + [3, 4]").message.contains("数値"), "配列の + は文字列連結になる（add を使う）");
    let mut l = solid("a");
    l.transform.position.expression = expr("5");
    let e = ExprEngine::new()
        .eval_property(&project(vec![l]), &CompId::new("c"), &LayerId::new("a"), "transform.position", Time::ZERO)
        .unwrap_err();
    assert!(e.message.contains("次元"), "{e}");
    // 無限ループは打ち切られないが、深すぎる再帰は検出される
    assert!(error_of("function f() { return f(); }\nf()").line.is_some());
}

#[test]
fn x2_circular_references_and_errors_in_referenced_expressions() {
    // 自己参照はエクスプレッション適用前の値
    let mut a = solid("a");
    a.transform.rotation = Property::constant(7.0);
    a.transform.rotation.expression = expr("thisLayer.rotation + 1");
    assert_eq!(eval(&project(vec![a]), "a", "transform.rotation", 0.0), vec![8.0]);
    // 相互参照（A→B→A）でもハングせず、内側は適用前の値を見る
    let mut a = solid("a");
    a.transform.rotation = Property::constant(1.0);
    a.transform.rotation.expression = expr("thisComp.layer(\"b\").rotation + 10");
    let mut b = solid("b");
    b.transform.rotation = Property::constant(2.0);
    b.transform.rotation.expression = expr("thisComp.layer(\"a\").rotation + 100");
    let p = project(vec![a, b]);
    assert_eq!(eval(&p, "a", "transform.rotation", 0.0), vec![111.0]);
    // 参照先のエラーは、参照先のレイヤー・行番号で報告される
    let mut a = solid("a");
    a.transform.rotation.expression = expr("thisComp.layer(\"b\").rotation");
    let mut b = solid("b");
    b.name = "壊れた式".into();
    b.transform.rotation.expression = expr("1 +\nnope");
    a.transform.rotation.expression = expr("thisComp.layer(\"壊れた式\").rotation");
    let p = project(vec![a, b]);
    let e = ExprEngine::new().eval_property(&p, &CompId::new("c"), &LayerId::new("a"), "transform.rotation", Time::ZERO).unwrap_err();
    assert_eq!((e.layer.as_str(), e.line), ("壊れた式", Some(2)), "{e}");
    let all = ExprEngine::new().check(&p, &CompId::new("c"), Time::ZERO);
    assert_eq!(all.len(), 2);
}

#[test]
fn r1_renderer_applies_expressions_including_effects_and_disabled_ones() {
    let mut l = Layer::new(
        "s",
        "s",
        LayerKind::Solid { width: 20, height: 20, color: [1.0, 1.0, 1.0] },
        Transform::centered(20.0, 20.0, 400.0, 300.0),
        s(10.0),
    );
    l.transform.position = Property::spatial([50.0, 150.0]);
    l.transform.position.expression = expr("[value[0] + time * 100, value[1]]");
    l.transform.opacity.expression = expr("50");
    let mut p = project(vec![l]);
    let engine = ExprEngine::new();
    let opts = RenderOptions { downsample: 1, background: false, guides: false };
    let render =
        |p: &Project, t: f64| Renderer::new(p, &NoFootage).with_resolver(&engine).render_frame(&CompId::new("c"), s(t), opts).unwrap();
    let out = render(&p, 2.0);
    assert_eq!(out.pixel(250, 150), [0.5; 4]);
    assert_eq!(out.pixel(50, 150), [0.0; 4]);
    // エクスプレッションを無効にすると元の値に戻る
    p.compositions[0].layers[0].transform.position.expression.as_mut().unwrap().enabled = false;
    engine.clear();
    assert_eq!(render(&p, 2.0).pixel(50, 150), [0.5; 4]);
    // リゾルバーなしでは式は無視される
    let plain = Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), s(2.0), opts).unwrap();
    assert_eq!(plain.pixel(50, 150), [1.0; 4]);
    // エラーのある式はレンダリングエラーになる（黙って無視しない）
    p.compositions[0].layers[0].transform.opacity.expression = expr("oops");
    engine.clear();
    let err = Renderer::new(&p, &NoFootage).with_resolver(&engine).render_frame(&CompId::new("c"), s(0.0), opts).unwrap_err();
    assert!(err.to_string().contains("oops") && err.to_string().contains("1 行目"), "{err}");
}

#[test]
fn r2_expression_selector_drives_text_animator() {
    use znd_core::text::{AnimatorProps, SelectorUnit};
    if znd_text::layout(&TextDocument::new("A", 50.0), None, &|_| 0.0, Time::ZERO).glyphs.is_empty() {
        eprintln!("フォントが無いためスキップ");
        return;
    }
    let animator = |src: &str| TextAnimator {
        name: "a".into(),
        selectors: vec![TextSelector::Expression { source: src.into(), unit: SelectorUnit::Characters }],
        props: AnimatorProps { opacity: Some(Property::constant(0.0)), ..Default::default() },
    };
    let ink = |src: &str| {
        let l = Layer::new(
            "t",
            "t",
            LayerKind::Text { document: TextDocument::new("AAAA", 60.0), animators: vec![animator(src)], path: None, per_char_3d: false },
            Transform::centered(0.0, 0.0, 200.0, 300.0),
            s(10.0),
        );
        let p = project(vec![l]);
        let engine = ExprEngine::new();
        let out = Renderer::new(&p, &NoFootage)
            .with_resolver(&engine)
            .render_frame(&CompId::new("c"), Time::ZERO, RenderOptions { downsample: 1, background: false, guides: false })
            .unwrap();
        out.data.chunks_exact(4).map(|p| p[3] as f64).sum::<f64>()
    };
    let full = ink("0");
    assert!(full > 1000.0);
    assert!(ink("100") < 1.0, "全文字が透明");
    // 偶数番目の文字だけ消す → インクは半分
    let half = ink("textIndex % 2 == 0 ? 100 : 0");
    assert!((half / full - 0.5).abs() < 0.02, "{}", half / full);
    // 後ろの文字ほど薄く: 25, 50, 75, 100% 消える → 残りは (75+50+25+0)/400
    let ramp = ink("textIndex / textTotal * 100");
    assert!((ramp / full - 0.375).abs() < 0.02, "{}", ramp / full);
}

#[test]
fn s1_automation_script_edits_project_with_validation() {
    let mut p = project(vec![solid("a"), solid("b")]);
    let logs = run_script(
        &mut p,
        r#"
        var c = comp('メイン');
        log('レイヤー数', c.layers.length);
        // 全レイヤーを 0.5 秒ずつずらして登場させ、回転にキーを打つ
        c.layers.forEach(function (l, i) {
          l.in_point = flicks(i * 0.5);
          setKeys(l.transform.rotation, [key(1, 90, 'ease'), key(0, 0, 'ease')]);
        });
        layer(c, 'b').name = '二番目';
        project.name = '一括処理済み';
        "#,
    )
    .unwrap();
    assert_eq!(logs, vec!["レイヤー数 2".to_string()]);
    assert_eq!(p.name, "一括処理済み");
    let c = &p.compositions[0];
    assert_eq!(c.layers[1].in_point, s(0.5));
    assert_eq!(c.layers[1].name, "二番目");
    assert_eq!(c.layers[0].transform.rotation.keyframes.len(), 2);
    assert!((c.layers[0].transform.rotation.eval(s(0.5)) - 45.0).abs() < 1e-6, "イージーイーズの中点");

    // 構造を壊すスクリプトは取り消される
    let before = p.clone();
    let err = run_script(&mut p, "layer('メイン', 'a').parent = '存在しない';").unwrap_err();
    assert!(err.contains("取り消しました") && err.contains("存在しない"), "{err}");
    assert_eq!(p, before);
    // 実行時エラーは行番号付き
    let err = run_script(&mut p, "var x = 1;\ncomp('無い');").unwrap_err();
    assert!(err.contains("無い") && err.contains("2 行目"), "{err}");
    assert_eq!(p, before);
    // 型を壊した場合
    let err = run_script(&mut p, "project.compositions = 5;").unwrap_err();
    assert!(err.contains("不正"), "{err}");
    assert_eq!(p, before);
}

#[test]
fn checkbox_and_choice_effect_params_are_readable() {
    let mut l = solid("a");
    l.effects.push(effect::EffectInstance::new("gaussian_blur", 100.0, 100.0).unwrap().set_b("repeat_edge", true).set_i("dimensions", 2));
    let name = l.effects[0].name.clone();
    l.transform.rotation.expression = expr(&format!("effect({name:?})(\"repeat_edge\") * 10 + effect({name:?})(\"dimensions\")"));
    let p = project(vec![l]);
    assert_eq!(eval(&p, "a", "transform.rotation", 0.0), vec![12.0]);
}

#[test]
fn library_functions_see_the_current_comp_and_time() {
    znd_render::plugins::hooks::add_expression_library("test/this.js", "function __testCompWidthPlusTime() { return thisComp.width + time + thisLayer.index; }");
    let mut l = solid("a");
    l.transform.rotation.expression = expr("__testCompWidthPlusTime()");
    let p = project(vec![l]);
    assert_eq!(eval(&p, "a", "transform.rotation", 2.0), vec![403.0]);
}
