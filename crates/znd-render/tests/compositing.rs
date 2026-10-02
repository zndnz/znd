//! M1 受け入れテスト（docs/05-acceptance-criteria.md の R 系列）。
//!
//! 解析的に期待値が決まるものは数値で、絵全体は参照画像（tests/golden）との比較で検証する。
//! 参照画像の更新: `ZND_BLESS=1 cargo test -p znd-render`

use std::path::PathBuf;
use znd_core::demo::demo_project;
use znd_core::*;
use znd_render::color::srgb_to_linear;
use znd_render::{to_rgba8, FrameBuf, NoFootage, RenderError, RenderOptions, Renderer};

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

fn dur() -> Time {
    Time::from_seconds(1.0)
}

fn comp(id: &str, w: u32, h: u32, layers: Vec<Layer>) -> Composition {
    Composition {
        id: CompId::new(id),
        name: id.into(),
        width: w,
        height: h,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        duration: dur(),
        background: [0.0; 3],
        layers,
        markers: vec![],
        ..Composition::blank()
    }
}

/// `comp` サイズ (cw, ch) の中央に置いた平面。
fn solid(id: &str, w: u32, h: u32, color: [f32; 3], cw: u32, ch: u32) -> Layer {
    Layer::new(
        id,
        id,
        LayerKind::Solid { width: w, height: h, color },
        Transform::centered(w as f64, h as f64, cw as f64, ch as f64),
        dur(),
    )
}

fn project(comps: Vec<Composition>) -> Project {
    let mut p = Project::new("test");
    p.compositions = comps;
    p
}

const ALPHA: RenderOptions = RenderOptions { downsample: 1, background: false, guides: true };

fn render(p: &Project, id: &str, opts: RenderOptions) -> FrameBuf {
    Renderer::new(p, &NoFootage).render_frame(&CompId::new(id), Time::ZERO, opts).unwrap()
}

fn assert_px(buf: &FrameBuf, x: usize, y: usize, expect: [f32; 4]) {
    let p = buf.pixel(x, y);
    assert!(p.iter().zip(&expect).all(|(a, b)| (a - b).abs() < 1e-4), "({x},{y}) = {p:?}, expected {expect:?}");
}

#[test]
fn r1_half_transparent_red_over_blue_is_mixed_in_linear_light() {
    let mut top = solid("top", 32, 32, RED, 64, 64);
    top.transform.opacity = Property::constant(50.0);
    let p = project(vec![comp("c", 64, 64, vec![top, solid("bottom", 64, 64, BLUE, 64, 64)])]);
    let out = render(&p, "c", ALPHA);
    assert_px(&out, 32, 32, [0.5, 0.0, 0.5, 1.0]);
    assert_px(&out, 2, 2, [0.0, 0.0, 1.0, 1.0]);
    // 表示値では (188, 0, 188)。リニア 0.5 は表示値 0.735
    assert_eq!(&to_rgba8(&out, None)[(32 * 64 + 32) * 4..][..4], &[188, 0, 188, 255]);

    // ガンマ空間合成に切り替えると表示値の平均 (128) になる
    let mut pg = p.clone();
    pg.settings.linear_blending = false;
    let px = to_rgba8(&render(&pg, "c", ALPHA), None)[(32 * 64 + 32) * 4..][..4].to_vec();
    assert!((127..=128).contains(&px[0]) && px[1] == 0 && px[0] == px[2] && px[3] == 255, "{px:?}");
}

#[test]
fn r2_background_and_alpha_output() {
    let mut c = comp("c", 8, 8, vec![solid("s", 4, 4, WHITE, 8, 8)]);
    c.background = [0.5, 0.5, 0.5];
    let p = project(vec![c]);
    assert_px(&render(&p, "c", ALPHA), 0, 0, [0.0; 4]);
    let g = srgb_to_linear(0.5);
    assert_px(&render(&p, "c", RenderOptions::EXPORT), 0, 0, [g, g, g, 1.0]);
    assert_px(&render(&p, "c", RenderOptions::EXPORT), 4, 4, [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn r3_parent_rotation_moves_child() {
    let mut null = Layer::new("null", "null", LayerKind::Null, Transform::centered(0.0, 0.0, 100.0, 100.0), dur());
    null.transform.rotation = Property::constant(90.0);
    let mut child = solid("child", 10, 10, WHITE, 0, 0);
    child.parent = Some(LayerId::new("null"));
    child.transform.position = Property::spatial([30.0, 0.0]);
    let p = project(vec![comp("c", 100, 100, vec![child, null])]);
    let out = render(&p, "c", ALPHA);
    // 親が 90° 回転しているので、子は中心の右ではなく下 30px に来る
    assert_px(&out, 50, 80, [1.0; 4]);
    assert_px(&out, 80, 50, [0.0; 4]);
}

#[test]
fn r4_keyframed_position_and_in_out_points() {
    let mut s = solid("s", 10, 10, WHITE, 100, 20);
    s.transform.position =
        Property::spatial([0.0, 0.0]).with_keys(vec![Keyframe::linear(Time::ZERO, [5.0, 10.0]), Keyframe::linear(dur(), [95.0, 10.0])]);
    s.in_point = Time::from_seconds(0.25);
    let p = project(vec![comp("c", 100, 20, vec![s])]);
    let r = Renderer::new(&p, &NoFootage);
    let at = |sec: f64| r.render_frame(&CompId::new("c"), Time::from_seconds(sec), ALPHA).unwrap();
    assert!(at(0.0).data.iter().all(|v| *v == 0.0), "イン点より前は描かれない");
    assert_px(&at(0.5), 50, 10, [1.0; 4]);
    assert_px(&at(0.5), 40, 10, [0.0; 4]);
}

#[test]
fn r5_track_mattes() {
    let build = |mode: MatteMode, matte_color: [f32; 3]| {
        let mut matte = solid("matte", 20, 40, matte_color, 40, 40);
        matte.visible = false;
        matte.transform.position = Property::spatial([10.0, 20.0]);
        let mut fill = solid("fill", 40, 40, RED, 40, 40);
        fill.track_matte = Some(TrackMatte { layer: LayerId::new("matte"), mode });
        render(&project(vec![comp("c", 40, 40, vec![matte, fill])]), "c", ALPHA)
    };
    let a = build(MatteMode::Alpha, WHITE);
    assert_px(&a, 5, 5, [1.0, 0.0, 0.0, 1.0]);
    assert_px(&a, 30, 5, [0.0; 4]);
    let ai = build(MatteMode::AlphaInverted, WHITE);
    assert_px(&ai, 5, 5, [0.0; 4]);
    assert_px(&ai, 30, 5, [1.0, 0.0, 0.0, 1.0]);
    // ルミナンスマット: 表示値 0.5 のグレー = リニア輝度 0.214
    let l = build(MatteMode::Luma, [0.5, 0.5, 0.5]);
    let g = srgb_to_linear(0.5);
    assert_px(&l, 5, 5, [g, 0.0, 0.0, g]);
    let li = build(MatteMode::LumaInverted, [0.5, 0.5, 0.5]);
    assert_px(&li, 5, 5, [1.0 - g, 0.0, 0.0, 1.0 - g]);
    assert_px(&li, 30, 5, [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn r6_solo_guide_and_visibility_switches() {
    let mut red = solid("red", 8, 8, RED, 8, 8);
    let mut blue = solid("blue", 8, 8, BLUE, 8, 8);
    let p = project(vec![comp("c", 8, 8, vec![red.clone(), blue.clone()])]);
    assert_px(&render(&p, "c", ALPHA), 4, 4, [1.0, 0.0, 0.0, 1.0]);

    blue.solo = true;
    let p = project(vec![comp("c", 8, 8, vec![red.clone(), blue.clone()])]);
    assert_px(&render(&p, "c", ALPHA), 4, 4, [0.0, 0.0, 1.0, 1.0]);

    blue.solo = false;
    red.guide = true;
    let p = project(vec![comp("c", 8, 8, vec![red.clone(), blue.clone()])]);
    assert_px(&render(&p, "c", RenderOptions::PREVIEW), 4, 4, [1.0, 0.0, 0.0, 1.0]);
    assert_px(&render(&p, "c", RenderOptions::EXPORT), 4, 4, [0.0, 0.0, 1.0, 1.0]);

    red.guide = false;
    red.visible = false;
    let p = project(vec![comp("c", 8, 8, vec![red, blue])]);
    assert_px(&render(&p, "c", ALPHA), 4, 4, [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn r7_precomp_matches_flattened_and_collapse_removes_crop() {
    // 40x40 のプリコンポ内で、平面が右に半分はみ出している
    let mut inner = solid("inner", 20, 20, RED, 40, 40);
    inner.transform.position = Property::spatial([40.0, 20.0]);
    let nested = comp("nested", 40, 40, vec![inner.clone()]);

    let pre = |collapse: bool| {
        let mut l = Layer::new(
            "pre",
            "pre",
            LayerKind::Precomp { comp: CompId::new("nested") },
            Transform::centered(40.0, 40.0, 80.0, 80.0),
            dur(),
        );
        l.collapse = collapse;
        l.transform.opacity = Property::constant(50.0);
        l
    };
    let plain = render(&project(vec![comp("c", 80, 80, vec![pre(false)]), nested.clone()]), "c", ALPHA);
    let collapsed = render(&project(vec![comp("c", 80, 80, vec![pre(true)]), nested.clone()]), "c", ALPHA);

    // プリコンポ内座標 (35,20) → 親 (55,40): どちらでも描かれる
    assert_px(&plain, 55, 40, [0.5, 0.0, 0.0, 0.5]);
    assert_px(&collapsed, 55, 40, [0.5, 0.0, 0.0, 0.5]);
    // プリコンポ内座標 (45,20) → 親 (65,40): 通常は境界で切られ、コラップスでは見える
    assert_px(&plain, 65, 40, [0.0; 4]);
    assert_px(&collapsed, 65, 40, [0.5, 0.0, 0.0, 0.5]);

    // プリコンポを使わずに同じ絵を直接組んだものと、コラップス結果は一致する
    let mut flat = inner;
    flat.transform.position = Property::spatial([60.0, 40.0]);
    flat.transform.opacity = Property::constant(50.0);
    let direct = render(&project(vec![comp("c", 80, 80, vec![flat])]), "c", ALPHA);
    assert_eq!(direct, collapsed);
}

#[test]
fn r8_cyclic_precomp_is_an_error_not_a_hang() {
    let l = Layer::new("self", "self", LayerKind::Precomp { comp: CompId::new("c") }, Transform::centered(8.0, 8.0, 8.0, 8.0), dur());
    let p = project(vec![comp("c", 8, 8, vec![l])]);
    assert!(!p.validate().is_empty());
    let err = Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), Time::ZERO, ALPHA);
    assert!(matches!(err, Err(RenderError::TooDeep)));
}

#[test]
fn r9_half_resolution_matches_downsampled_full_resolution() {
    let p = demo_project();
    let r = Renderer::new(&p, &NoFootage);
    let t = Time::from_seconds(1.2);
    let full = r.render_frame(&CompId::new("main"), t, RenderOptions::EXPORT).unwrap();
    let half = r.render_frame(&CompId::new("main"), t, RenderOptions { downsample: 2, ..RenderOptions::EXPORT }).unwrap();
    assert_eq!((half.width, half.height), (960, 540));
    // フル解像度を 2x2 ボックスで縮小したものと比較
    let mut sum_sq = 0.0f64;
    for y in 0..half.height {
        for x in 0..half.width {
            let h = half.pixel(x, y);
            for (c, hc) in h.iter().enumerate().take(3) {
                let avg = (full.pixel(2 * x, 2 * y)[c]
                    + full.pixel(2 * x + 1, 2 * y)[c]
                    + full.pixel(2 * x, 2 * y + 1)[c]
                    + full.pixel(2 * x + 1, 2 * y + 1)[c])
                    / 4.0;
                sum_sq += ((hc - avg) as f64).powi(2);
            }
        }
    }
    let rmse = (sum_sq / (half.width * half.height * 3) as f64).sqrt();
    assert!(rmse < 0.01, "rmse {rmse}");
}

/// 参照画像比較。許容: 8bit sRGB で最大差 2、RMSE 0.25 以下。
fn golden(name: &str, buf: &FrameBuf) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.png"));
    let actual = to_rgba8(buf, None);
    if std::env::var_os("ZND_BLESS").is_some() {
        image::save_buffer(&path, &actual, buf.width as u32, buf.height as u32, image::ExtendedColorType::Rgba8).unwrap();
        return;
    }
    let expected = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}（ZND_BLESS=1 で生成）", path.display())).to_rgba8();
    assert_eq!((expected.width() as usize, expected.height() as usize), (buf.width, buf.height), "{name}: size");
    let mut max = 0i32;
    let mut sum_sq = 0f64;
    for (a, e) in actual.iter().zip(expected.as_raw()) {
        let d = (*a as i32 - *e as i32).abs();
        max = max.max(d);
        sum_sq += (d * d) as f64;
    }
    let rmse = (sum_sq / actual.len() as f64).sqrt();
    if max > 2 || rmse > 0.25 {
        let failed = path.with_extension("actual.png");
        image::save_buffer(&failed, &actual, buf.width as u32, buf.height as u32, image::ExtendedColorType::Rgba8).unwrap();
        panic!("{name}: 参照画像と不一致（最大差 {max}, RMSE {rmse:.3}）。出力: {}", failed.display());
    }
}

#[test]
fn r10_demo_frames_match_golden_images() {
    let p = demo_project();
    let r = Renderer::new(&p, &NoFootage);
    let comp = p.comp(&CompId::new("main")).unwrap();
    for frame in [0i64, 20, 45, 100, 149] {
        let t = comp.frame_rate.frame_to_time(frame);
        let buf = r.render_frame(&comp.id, t, RenderOptions { downsample: 2, ..RenderOptions::EXPORT }).unwrap();
        golden(&format!("demo_main_{frame:04}"), &buf);
    }
}

#[test]
fn r11_all_blend_modes_match_golden_image() {
    // 31 モードを 1 枚に並べる。各セルは 3 色の帯の上に、回転した半透明の平面を重ねたもの。
    // （ステンシル系は下の絵全体に作用するので、セルごとに別コンポとして描く）
    let cols = 8usize;
    let cell = 48u32;
    let rows = BlendMode::ALL.len().div_ceil(cols);
    let mut atlas = FrameBuf::new(cols * cell as usize, rows * cell as usize);
    for (i, mode) in BlendMode::ALL.iter().enumerate() {
        let mut top = solid("top", 28, 28, [0.9, 0.6, 0.2], cell, cell);
        top.transform.rotation = Property::constant(20.0);
        top.transform.opacity = Property::constant(80.0);
        top.blend_mode = *mode;
        let mut layers = vec![top];
        for (b, color) in [[0.1, 0.3, 0.8], [0.8, 0.8, 0.8], [0.3, 0.7, 0.3]].iter().enumerate() {
            let mut band = solid(&format!("band-{b}"), cell, 16, *color, cell, cell);
            band.transform.position = Property::spatial([cell as f64 / 2.0, 8.0 + 16.0 * b as f64]);
            layers.push(band);
        }
        let out = render(&project(vec![comp("c", cell, cell, layers)]), "c", ALPHA);
        let (ox, oy) = ((i % cols) * cell as usize, (i / cols) * cell as usize);
        for y in 0..cell as usize {
            for x in 0..cell as usize {
                atlas.set_pixel(ox + x, oy + y, out.pixel(x, y));
            }
        }
    }
    golden("blend_modes", &atlas);
}
