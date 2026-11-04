//! 自動化スクリプトの補助関数（addEffect / setParam / findEffect）。

#[test]
fn add_effect_creates_defaults_and_set_param_overrides() {
    let mut p = znd_core::demo::demo_project();
    let comp = p.compositions[0].id.0.clone();
    let src = format!(
        "var c = comp('{comp}'); var l = c.layers[0];\n\
         var fx = addEffect(l, 'gaussian_blur', 'ぼかし');\n\
         setParam(fx, 'radius', 25); setParam(fx, 'repeat_edge', true);\n\
         if (findEffect(l, 'ぼかし') !== fx) throw new Error('not found');\n\
         var threw = false; try {{ addEffect(l, 'no_such_effect'); }} catch (e) {{ threw = true; }}\n\
         if (!threw) throw new Error('should throw');"
    );
    znd_script::run_script(&mut p, &src).unwrap();
    let fx = p.compositions[0].layers[0].effects.last().unwrap();
    assert_eq!((fx.kind.as_str(), fx.name.as_str()), ("gaussian_blur", "ぼかし"));
    let e = fx.eval(znd_core::Time::ZERO);
    assert_eq!((e.f("radius"), e.b("repeat_edge"), e.i("dimensions")), (25.0, true, 0));
}

#[test]
fn add_solid_creates_a_valid_layer_on_top() {
    let mut p = znd_core::demo::demo_project();
    let comp = p.compositions[0].id.0.clone();
    let n = p.compositions[0].layers.len();
    znd_script::run_script(&mut p, &format!("var c = comp('{comp}'); var l = addSolid(c, '線', [0, 0, 0]); addEffect(l, 'gaussian_blur');")).unwrap();
    let c = &p.compositions[0];
    assert_eq!(c.layers.len(), n + 1);
    assert_eq!(c.layers[0].name, "線");
    assert!(matches!(c.layers[0].kind, znd_core::LayerKind::Solid { width, height, .. } if width == c.width && height == c.height));
    assert_eq!(c.layers[0].out_point, c.duration);
    assert!(p.validate().is_empty());
}

#[test]
fn add_text_creates_a_centered_text_layer_on_top() {
    let mut p = znd_core::demo::demo_project();
    let comp = p.compositions[0].id.0.clone();
    let n = p.compositions[0].layers.len();
    znd_script::run_script(&mut p, &format!("var c = comp('{comp}'); var l = addText(c, '字幕', 'こんにちは', 48);")).unwrap();
    let c = &p.compositions[0];
    assert_eq!(c.layers.len(), n + 1);
    let znd_core::LayerKind::Text { document, .. } = &c.layers[0].kind else { panic!("テキストレイヤーではない") };
    assert_eq!((document.text.as_str(), document.size), ("こんにちは", 48.0));
    assert_eq!(c.layers[0].name, "字幕");
    assert!(p.validate().is_empty());
}
