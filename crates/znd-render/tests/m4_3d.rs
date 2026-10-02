//! M4 受け入れテスト（描画側）: 3D レイヤー、カメラ、ライト、影、3D モデル、パーティクル。

mod common;

use common::*;
use znd_core::*;
use znd_render::three_d::default_zoom;
use znd_render::{FrameBuf, NoFootage, Renderer};

const W: u32 = 400;
const H: u32 = 300;

fn p1(v: f64) -> Property<f64> {
    Property::constant(v)
}

fn solid3d(id: &str, w: u32, h: u32, color: [f32; 3]) -> Layer {
    let mut l = solid(id, w, h, color, W, H);
    l.three_d = true;
    l
}

fn camera(zoom: f64) -> Layer {
    let mut tr = Transform::centered(0.0, 0.0, W as f64, H as f64);
    tr.position_z = p1(-zoom);
    Layer::new(
        "cam",
        "cam",
        LayerKind::Camera {
            zoom: p1(zoom),
            point_of_interest: Some(Property::constant([W as f64 / 2.0, H as f64 / 2.0, 0.0])),
            depth_of_field: false,
            focus_distance: p1(zoom),
            aperture: p1(0.0),
            iris_sides: 0,
        },
        tr,
        dur(),
    )
}

fn light(kind: LightType, pos: [f64; 3], intensity: f64) -> Layer {
    let mut tr = Transform::centered(0.0, 0.0, 0.0, 0.0);
    tr.position = Property::spatial([pos[0], pos[1]]);
    tr.position_z = p1(pos[2]);
    Layer::new(
        format!("light-{kind:?}"),
        "light",
        LayerKind::Light {
            light: kind,
            color: [1.0, 1.0, 1.0],
            intensity: p1(intensity),
            cone_angle: p1(90.0),
            cone_feather: p1(50.0),
            casts_shadows: false,
            shadow_darkness: 100.0,
            shadow_diffusion: 0.0,
        },
        tr,
        dur(),
    )
}

fn run(layers: Vec<Layer>) -> FrameBuf {
    render(&project(vec![comp("c", W, H, layers)]), "c")
}

fn zoom() -> f64 {
    default_zoom(&comp("c", W, H, vec![]))
}

#[test]
fn d1_3d_layer_at_z0_matches_2d_layer() {
    let flat = run(vec![solid("s", 100, 60, RED, W, H)]);
    let three = run(vec![solid3d("s", 100, 60, RED)]);
    assert!((alpha_sum(&flat) - alpha_sum(&three)).abs() < 1.0);
    assert_px(&three, 200, 150, [1.0, 0.0, 0.0, 1.0], 1e-4);
    assert_px(&three, 152, 122, [1.0, 0.0, 0.0, 1.0], 1e-4);
    assert_px(&three, 148, 150, [0.0; 4], 1e-4);
    // 明示的にカメラを置いても（既定と同じ位置・ズームなら）同じ絵
    let with_cam = run(vec![camera(zoom()), solid3d("s", 100, 60, RED)]);
    assert!((alpha_sum(&with_cam) - alpha_sum(&flat)).abs() < 1.0);
}

#[test]
fn d2_perspective_scales_with_distance_and_rotation_foreshortens() {
    let base = alpha_sum(&run(vec![solid3d("s", 100, 60, RED)]));
    // カメラからの距離が 2 倍（Z = ズーム値）→ 大きさ 1/2、面積 1/4
    let mut far = solid3d("s", 100, 60, RED);
    far.transform.position_z = p1(zoom());
    assert!((alpha_sum(&run(vec![far])) / base - 0.25).abs() < 0.01);
    // 手前に半分の距離 → 面積 4 倍（画面内に収まる大きさ）
    let mut near = solid3d("s", 40, 30, RED);
    near.transform.position_z = p1(-zoom() / 2.0);
    let small = alpha_sum(&run(vec![solid3d("s", 40, 30, RED)]));
    assert!((alpha_sum(&run(vec![near])) / small - 4.0).abs() < 0.05);
    // Y 回転 60° → 幅がほぼ半分に見える（遠近で僅かにずれる）
    let mut turned = solid3d("s", 100, 60, RED);
    turned.transform.rotation_y = p1(60.0);
    let ratio = alpha_sum(&run(vec![turned.clone()])) / base;
    assert!((ratio - 0.5).abs() < 0.03, "{ratio}");
    // 右端が奥へ行くので、右側の辺のほうが短く見える
    let out = run(vec![turned]);
    let col_height = |x: usize| (0..H as usize).filter(|y| out.pixel(x, *y)[3] > 0.5).count();
    assert!(col_height(180) > col_height(220), "{} vs {}", col_height(180), col_height(220));
    // 真横（90°）は見えない
    let mut edge = solid3d("s", 100, 60, RED);
    edge.transform.rotation_y = p1(90.0);
    assert!(alpha_sum(&run(vec![edge])) < 1.0);
}

#[test]
fn d3_camera_movement_and_two_node_orientation() {
    // カメラを右へ 50px 平行移動（目標点も一緒に）→ Z=0 の絵は左へ 50px
    let mut cam = camera(zoom());
    cam.transform.position = Property::spatial([250.0, 150.0]);
    if let LayerKind::Camera { point_of_interest, .. } = &mut cam.kind {
        *point_of_interest = Some(Property::constant([250.0, 150.0, 0.0]));
    }
    let out = run(vec![cam, solid3d("s", 40, 40, RED)]);
    let (cx, cy) = centroid(&out);
    assert!((cx - 150.0).abs() < 0.5 && (cy - 150.0).abs() < 0.5, "{cx},{cy}");

    // カメラを右へ動かし、目標点は中央のまま（回り込み）→ 中央の物体は画面中央に留まる
    let mut cam = camera(zoom());
    cam.transform.position = Property::spatial([400.0, 150.0]);
    let out = run(vec![cam, solid3d("s", 40, 40, RED)]);
    let (cx, _) = centroid(&out);
    assert!((cx - 200.0).abs() < 2.0, "{cx}");

    // ズームを 2 倍 → Z=0 の絵は 2 倍に拡大（カメラ位置は既定のまま）
    let mut cam = camera(zoom());
    if let LayerKind::Camera { zoom: z, .. } = &mut cam.kind {
        *z = p1(zoom() * 2.0);
    }
    let big = alpha_sum(&run(vec![cam, solid3d("s", 40, 40, RED)]));
    assert!((big / 1600.0 - 4.0).abs() < 0.05, "{}", big / 1600.0);
}

#[test]
fn d4_depth_order_ignores_layer_order_and_2d_layers_split_runs() {
    // レイヤー順では赤が上だが、赤のほうが奥にある → 緑が手前に描かれる
    let mut red = solid3d("red", 100, 100, RED);
    red.transform.position_z = p1(200.0);
    let green = solid3d("green", 60, 60, GREEN);
    let out = run(vec![red.clone(), green.clone()]);
    assert_px(&out, 200, 150, [0.0, 1.0, 0.0, 1.0], 1e-4);
    // 間に 2D レイヤーを挟むと 3D のまとまりが分かれ、レイヤー順どおりになる
    let mut blue2d = solid("blue", 20, 20, BLUE, W, H);
    blue2d.transform.opacity = p1(100.0);
    let out = run(vec![red, blue2d, green]);
    assert_px(&out, 200, 150, [1.0, 0.0, 0.0, 1.0], 1e-4);
}

#[test]
fn d5_lights_shade_3d_layers() {
    let lit = |layers: Vec<Layer>| run(layers).pixel(200, 150);
    // ライトが無ければそのままの色
    assert_eq!(lit(vec![solid3d("s", 200, 200, WHITE)])[0], 1.0);
    // 2D レイヤーはライトの影響を受けない
    let px = lit(vec![light(LightType::Ambient, [0.0, 0.0, 0.0], 30.0), solid("s", 200, 200, WHITE, W, H)]);
    assert!((px[0] - 1.0).abs() < 1e-5);
    // アンビエント 30%
    let px = lit(vec![light(LightType::Ambient, [0.0, 0.0, 0.0], 30.0), solid3d("s", 200, 200, WHITE)]);
    assert!((px[0] - 0.3).abs() < 1e-3, "{px:?}");
    // 正面のポイントライト 100% → 中心は 1.0、斜めになる端は暗い
    let out = run(vec![light(LightType::Point, [200.0, 150.0, -200.0], 100.0), solid3d("s", 300, 200, WHITE)]);
    assert!((out.pixel(200, 150)[0] - 1.0).abs() < 1e-3);
    // x=340: 中心から 140px。cos = 200/√(200²+140²) = 0.819
    assert!((out.pixel(340, 150)[0] - 0.819).abs() < 0.01, "{}", out.pixel(340, 150)[0]);
    // ライトを受けない設定のレイヤーは暗くならない
    let mut unlit = solid3d("s", 200, 200, WHITE);
    unlit.accepts_lights = false;
    assert_eq!(lit(vec![light(LightType::Ambient, [0.0, 0.0, 0.0], 30.0), unlit])[0], 1.0);
    // スポットライト: 円錐の外は真っ暗
    let mut spot = light(LightType::Spot, [200.0, 150.0, -200.0], 100.0);
    if let LayerKind::Light { cone_angle, cone_feather, .. } = &mut spot.kind {
        *cone_angle = p1(40.0);
        *cone_feather = p1(0.0);
    }
    let out = run(vec![spot, solid3d("s", 380, 280, WHITE)]);
    assert!(out.pixel(200, 150)[0] > 0.99);
    // 半角 20° → 半径 200·tan20° = 72.8px。その外側は 0
    assert!(out.pixel(200 + 60, 150)[0] > 0.5);
    assert!(out.pixel(200 + 90, 150)[0] < 1e-3);
}

#[test]
fn d6_shadows_fall_on_layers_behind() {
    // 光源を左へずらし、影が遮蔽物の右側にはみ出すようにする
    let mut l = light(LightType::Point, [100.0, 150.0, -400.0], 100.0);
    if let LayerKind::Light { casts_shadows, shadow_darkness, .. } = &mut l.kind {
        *casts_shadows = true;
        *shadow_darkness = 80.0;
    }
    let mut caster = solid3d("caster", 40, 40, RED);
    caster.transform.position_z = p1(-100.0);
    caster.casts_shadows = true;
    let wall = solid3d("wall", 380, 280, WHITE);
    let out = run(vec![l.clone(), caster.clone(), wall.clone()]);
    let unshadowed = run(vec![l, wall]);
    // 光源 (100, 150, -400)、遮蔽物は (200, 150, -100) の一辺 40 → 壁 z=0 上の影は
    // 中心 x = 100 + 100·400/300 = 233.3、一辺 40·400/300 = 53.3（x 206.7..260）。
    // 遮蔽物自身は画面上 x 175.6..224.4 を覆うので、x=245 は「影の中で、遮蔽物には隠れていない」場所
    let (inside, outside) = (out.pixel(245, 150), out.pixel(285, 150));
    let reference = unshadowed.pixel(245, 150);
    assert!((inside[0] / reference[0] - 0.2).abs() < 0.02, "{inside:?} / {reference:?}");
    assert!((outside[0] - unshadowed.pixel(285, 150)[0]).abs() < 1e-4);
    // 影の上下の端: y は 150 ± 26.7
    assert!((out.pixel(245, 150 + 22)[0] / reference[0] - 0.2).abs() < 0.03);
    assert!(out.pixel(245, 150 + 32)[0] > 0.9 * unshadowed.pixel(245, 182)[0]);
    // 遮蔽物自身は影にならず、赤いまま（Z=-100 は距離 3/4 相当で少し大きく見える）
    assert!(out.pixel(200, 150)[0] > 0.9 && out.pixel(200, 150)[1] < 1e-3);
}

#[test]
fn d7_depth_of_field_blurs_out_of_focus_layers() {
    let cam = |aperture: f64| {
        let mut c = camera(zoom());
        if let LayerKind::Camera { depth_of_field, aperture: a, focus_distance, .. } = &mut c.kind {
            *depth_of_field = true;
            *a = p1(aperture);
            *focus_distance = p1(zoom());
        }
        c
    };
    let mut far = solid3d("far", 60, 60, WHITE);
    far.transform.position_z = p1(400.0);
    let max_alpha = |b: &FrameBuf| b.data.chunks_exact(4).map(|p| p[3]).fold(0.0f32, f32::max);
    // 合焦距離にあるレイヤーはぼけない
    let focused = run(vec![cam(60.0), solid3d("s", 60, 60, WHITE)]);
    assert!((alpha_sum(&focused) - 3600.0).abs() < 2.0);
    assert_px(&focused, 172, 150, [1.0; 4], 1e-3);
    // 奥のレイヤーはぼける（総量は保たれ、輪郭が広がる）
    let sharp = run(vec![cam(0.0), far.clone()]);
    let blurred = run(vec![cam(60.0), far]);
    assert!((alpha_sum(&sharp) - alpha_sum(&blurred)).abs() / alpha_sum(&sharp) < 0.02);
    let wide = |b: &FrameBuf| b.data.chunks_exact(4).filter(|p| p[3] > 0.02).count();
    assert!(wide(&blurred) as f64 > wide(&sharp) as f64 * 1.3);
    assert!(max_alpha(&sharp) > 0.99);
}

const CUBE: &str = "\
v -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1\nv -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\n\
f 1 2 3 4\nf 5 8 7 6\nf 1 5 6 2\nf 2 6 7 3\nf 3 7 8 4\nf 5 1 4 8\n";

fn model_project(extra: Vec<Layer>, rot_y: f64) -> (Project, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("znd-model-{}-{}", std::process::id(), rot_y as i64 + extra.len() as i64 * 1000));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cube.obj"), CUBE).unwrap();
    let mut p = Project::new("m");
    p.footage.push(FootageItem {
        id: FootageId::new("cube"),
        name: "cube.obj".into(),
        path: dir.join("cube.obj").display().to_string(),
        width: 0,
        height: 0,
        frame_rate: None,
        duration: None,
        rights: Rights::default(),
        audio: None,
        vfr: false,
    });
    let mut m = Layer::new(
        "model",
        "model",
        LayerKind::Model { footage: FootageId::new("cube") },
        Transform::centered(0.0, 0.0, W as f64, H as f64),
        dur(),
    );
    m.transform.rotation_y = p1(rot_y);
    let mut layers = vec![m];
    layers.extend(extra);
    p.compositions.push(comp("c", W, H, layers));
    (p, dir)
}

#[test]
fn d8_model_renders_shaded_and_composites_by_depth() {
    let (p, dir) = model_project(vec![], 0.0);
    let out = Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), Time::ZERO, ALPHA).unwrap();
    // 立方体は「最大寸法 = コンポ高さの半分（150px）」に正規化される。正面の面は手前（z=-75）なので少し大きく見える
    let k = zoom() / (zoom() - 75.0);
    let expect = (150.0 * k) * (150.0 * k);
    assert!((alpha_sum(&out) - expect).abs() / expect < 0.02, "{} vs {expect}", alpha_sum(&out));
    assert_px(&out, 200, 150, [0.8 * 1.25, 0.8 * 1.25, 0.8 * 1.25, 1.0], 0.08);
    // 回転させると 2 面が見え、面ごとに明るさが違う（シェーディング）
    let (p, dir2) = model_project(vec![], 35.0);
    let out = Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), Time::ZERO, ALPHA).unwrap();
    let (left, right) = (out.pixel(160, 150)[0], out.pixel(270, 150)[0]);
    assert!(left > 0.05 && right > 0.05 && (left - right).abs() > 0.05, "{left} {right}");

    // モデルを貫く平面: 手前半分はモデル、奥半分は平面に隠れる（画素ごとの深度比較）
    let mut plane = solid3d("plane", 300, 300, BLUE);
    plane.transform.rotation_y = p1(90.0 - 1e-3);
    // ほぼ真横を向いた平面は見えないので、45° に傾けて中心を貫かせる
    plane.transform.rotation_y = p1(45.0);
    let (p, dir3) = model_project(vec![plane], 0.0);
    let out = Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), Time::ZERO, ALPHA).unwrap();
    // 平面は左側が手前・右側が奥。モデルの正面（z=-75）より手前に出るのは左側だけ
    let is_blue = |x: usize| out.pixel(x, 150)[2] > 0.5 && out.pixel(x, 150)[0] < 0.3;
    assert!(is_blue(110), "左端は平面が手前: {:?}", out.pixel(110, 150));
    assert!(!is_blue(230), "中央右はモデルが手前: {:?}", out.pixel(230, 150));
    for d in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn k1_particles_are_deterministic_and_obey_gravity() {
    let make = |gravity: f64, seconds: f64| {
        let mut l = solid("p", W, H, [0.0, 0.0, 0.0], W, H);
        l.effects = vec![fx("particles", W, H)
            .set_f("rate", 400.0)
            .set_f("life", 1.0)
            .set_f("speed", 130.0)
            .set_f("speed_random", 0.0)
            .set_f("gravity", gravity)
            .set_f("size", 1.5)
            .set_f("size_end", 100.0)
            .set_f("opacity_end", 100.0)
            .set_b("additive", false)];
        render_at(&project(vec![comp("c", W, H, vec![l])]), "c", seconds, ALPHA)
    };
    // 同じ時刻なら毎回同じ絵（どのフレームから描いても同じ）
    assert_eq!(make(100.0, 0.7), make(100.0, 0.7));
    assert_ne!(make(100.0, 0.7), make(100.0, 0.8));
    // 重力なし: 全方位に均等に広がるので重心は発生点（中央）
    let (cx, cy) = centroid(&make(0.0, 1.5));
    assert!((cx - 200.0).abs() < 5.0 && (cy - 150.0).abs() < 5.0, "{cx},{cy}");
    // 重力 g: 寿命 1 秒の粒子の平均落下量は g·⟨a²⟩/2 = g/6（年齢 a は 0..1 に一様）
    let (_, cy) = centroid(&make(120.0, 1.5));
    assert!((cy - 150.0 - 20.0).abs() < 5.0, "{cy}");
    // 発生数: rate × life 個が生きている。重なりを除くと面積はおおよそ個数 × 粒子面積
    let area = alpha_sum(&make(0.0, 1.5));
    let ideal = 400.0 * std::f64::consts::PI * 0.75 * 0.75;
    assert!(area > ideal * 0.85 && area < ideal * 1.05, "{area} vs {ideal}");
}

#[test]
fn k2_particles_follow_animated_emitter_and_bounce_on_floor() {
    // エミッターが左から右へ動く → 古い粒子は左に、新しい粒子は右に残る
    let mut l = solid("p", W, H, [0.0, 0.0, 0.0], W, H);
    let mut e = fx("particles", W, H)
        .set_f("rate", 100.0)
        .set_f("life", 1.0)
        .set_f("speed", 0.0)
        .set_f("gravity", 0.0)
        .set_f("size", 4.0)
        .set_f("size_end", 100.0)
        .set_f("opacity_end", 100.0);
    e.params.insert(
        "position".into(),
        ParamValue::Point(
            Property::constant([0.0, 0.0])
                .with_keys(vec![Keyframe::linear(Time::ZERO, [50.0, 150.0]), Keyframe::linear(Time::from_seconds(1.0), [350.0, 150.0])]),
        ),
    );
    l.effects = vec![e];
    let out = render_at(&project(vec![comp("c", W, H, vec![l])]), "c", 1.0, ALPHA);
    let cols: Vec<usize> = (0..W as usize).filter(|x| out.pixel(*x, 150)[3] > 0.3).collect();
    assert!(*cols.first().unwrap() < 60 && *cols.last().unwrap() > 340, "{:?}..{:?}", cols.first(), cols.last());

    // 床: 下向きに撃ち出しても床（y=200）より下には行かない
    let mut l = solid("p", W, H, [0.0, 0.0, 0.0], W, H);
    l.effects = vec![fx("particles", W, H)
        .set_f("rate", 150.0)
        .set_f("life", 2.0)
        .set_f("speed", 150.0)
        .set_f("direction", 90.0)
        .set_f("spread", 40.0)
        .set_f("gravity", 400.0)
        .set_b("floor_enabled", true)
        .set_f("floor_y", 200.0)
        .set_f("size", 4.0)
        .set_f("size_end", 100.0)
        .set_f("opacity_end", 100.0)];
    let out = render_at(&project(vec![comp("c", W, H, vec![l])]), "c", 1.9, ALPHA);
    let below: f64 = (204..H as usize).map(|y| (0..W as usize).map(|x| out.pixel(x, y)[3] as f64).sum::<f64>()).sum();
    assert!(below < 1e-3, "床より下に粒子がある: {below}");
    assert!(alpha_sum(&out) > 500.0);
}
