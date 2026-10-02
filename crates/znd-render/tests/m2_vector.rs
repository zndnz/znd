//! M2 受け入れテスト（描画側）: シェイプ・マスク・テキスト・時間操作・調整レイヤー。

mod common;

use common::*;
use std::sync::Arc;
use znd_core::shape::{FillRule, Gradient, MergeMode, ShapeTransform};
use znd_core::text::{AnimatorProps, Justify, TextPath};
use znd_core::*;
use znd_render::{FootageSource, FrameBuf, RenderOptions, Renderer};

fn p1(v: f64) -> Property<f64> {
    Property::constant(v)
}

#[test]
fn s1_shape_layer_composites_with_transform_and_blend() {
    // 60x40 の赤い矩形を、レイヤーのスケール 200%・位置 (+10, 0) で置く
    let mut l = shape("s", vec![ShapeItem::rect(60.0, 40.0), ShapeItem::fill([1.0, 0.0, 0.0, 1.0])], 200, 200);
    l.transform.scale = Property::constant([200.0, 200.0]);
    l.transform.position = Property::spatial([110.0, 100.0]);
    l.transform.opacity = Property::constant(50.0);
    let p = project(vec![comp("c", 200, 200, vec![l, solid("bg", 200, 200, BLUE, 200, 200)])]);
    let out = render(&p, "c");
    // 矩形は x 50..170, y 60..140 を覆う。50% なので赤と青が半々
    assert_px(&out, 110, 100, [0.5, 0.0, 0.5, 1.0], 1e-4);
    assert_px(&out, 51, 61, [0.5, 0.0, 0.5, 1.0], 1e-4);
    assert_px(&out, 48, 100, [0.0, 0.0, 1.0, 1.0], 1e-4);
    assert_px(&out, 110, 142, [0.0, 0.0, 1.0, 1.0], 1e-4);
    // 1/2 解像度でも同じ位置関係
    let half = render_at(&p, "c", 0.0, RenderOptions { downsample: 2, ..ALPHA });
    assert_px(&half, 55, 50, [0.5, 0.0, 0.5, 1.0], 1e-4);
    assert_px(&half, 23, 50, [0.0, 0.0, 1.0, 1.0], 1e-4);
}

#[test]
fn s2_shape_gallery_matches_golden() {
    let stroke = |c: [f64; 4], w: f64| ShapeItem::stroke(c, w);
    let fill = |c: [f64; 4]| ShapeItem::fill(c);
    let yellow = [1.0, 0.85, 0.2, 1.0];
    let cyan = [0.2, 0.8, 1.0, 1.0];
    let pink = [1.0, 0.3, 0.6, 1.0];
    let dashed = match stroke(cyan, 4.0) {
        ShapeItem::Stroke { color, opacity, width, miter_limit, dash_offset, gradient, .. } => ShapeItem::Stroke {
            color,
            opacity,
            width,
            cap: shape::LineCap::Round,
            join: shape::LineJoin::Round,
            miter_limit,
            dashes: vec![10.0, 8.0],
            dash_offset,
            gradient,
        },
        _ => unreachable!(),
    };
    let grad = Gradient {
        radial: true,
        start: Property::constant([0.0, 0.0]),
        end: Property::constant([36.0, 0.0]),
        stops: vec![(0.0, [1.0, 1.0, 1.0, 1.0]), (0.6, [1.0, 0.4, 0.1, 1.0]), (1.0, [0.3, 0.0, 0.4, 1.0])],
    };
    let cells: Vec<Vec<ShapeItem>> = vec![
        vec![ShapeItem::star(5.0, 36.0, 15.0), stroke(cyan, 3.0), fill(yellow)],
        vec![ShapeItem::polygon(6.0, 34.0), ShapeItem::RoundCorners { radius: p1(10.0) }, fill(pink)],
        vec![ShapeItem::ellipse(70.0, 50.0), dashed],
        vec![
            ShapeItem::ellipse(72.0, 72.0),
            ShapeItem::Fill { color: Property::constant([1.0; 4]), opacity: p1(100.0), rule: FillRule::NonZero, gradient: Some(grad) },
        ],
        vec![ShapeItem::ellipse(64.0, 64.0), ShapeItem::Trim { start: p1(10.0), end: p1(70.0), offset: p1(45.0) }, stroke(yellow, 6.0)],
        vec![
            ShapeItem::rect(14.0, 14.0),
            fill(cyan),
            ShapeItem::Repeater {
                copies: p1(6.0),
                offset: p1(-2.5),
                transform: ShapeTransform {
                    position: Property::constant([14.0, 0.0]),
                    rotation: p1(18.0),
                    scale: Property::constant([92.0, 92.0]),
                    ..Default::default()
                },
                end_opacity: p1(20.0),
            },
        ],
        vec![ShapeItem::polygon(5.0, 30.0), ShapeItem::PuckerBloat { amount: p1(60.0) }, fill(pink)],
        vec![ShapeItem::polygon(5.0, 30.0), ShapeItem::PuckerBloat { amount: p1(-60.0) }, fill(yellow)],
        vec![ShapeItem::ellipse(60.0, 60.0), ShapeItem::ZigZag { size: p1(5.0), ridges: p1(5.0), smooth: false }, stroke(cyan, 2.5)],
        vec![ShapeItem::ellipse(60.0, 60.0), ShapeItem::ZigZag { size: p1(5.0), ridges: p1(5.0), smooth: true }, stroke(pink, 2.5)],
        vec![ShapeItem::rect(64.0, 44.0), ShapeItem::Wave { amplitude: p1(4.0), wavelength: p1(22.0), phase: p1(0.0) }, fill(cyan)],
        vec![ShapeItem::rect(60.0, 60.0), ShapeItem::Twist { angle: p1(120.0), center: Property::constant([0.0, 0.0]) }, fill(yellow)],
        vec![ShapeItem::rect(50.0, 50.0), ShapeItem::OffsetPath { amount: p1(8.0), join: Default::default() }, stroke(pink, 2.0)],
        vec![
            ShapeItem::ellipse(50.0, 50.0),
            ShapeItem::Ellipse { size: Property::constant([50.0, 50.0]), position: Property::constant([22.0, 0.0]) },
            ShapeItem::Merge { mode: MergeMode::Exclude },
            fill(cyan),
        ],
        vec![
            ShapeItem::rect(60.0, 60.0),
            ShapeItem::WigglePath { size: p1(5.0), detail: p1(10.0), speed: p1(0.0), seed: 7 },
            stroke(yellow, 2.5),
        ],
        vec![
            ShapeItem::rect(70.0, 70.0),
            ShapeItem::ellipse(40.0, 40.0),
            ShapeItem::Fill { color: Property::constant(pink), opacity: p1(100.0), rule: FillRule::EvenOdd, gradient: None },
        ],
    ];
    let rendered: Vec<FrameBuf> =
        cells.into_iter().map(|items| render(&project(vec![comp("c", 96, 96, vec![shape("s", items, 96, 96)])]), "c")).collect();
    golden("shapes", &atlas(&rendered, 8));
}

#[test]
fn k1_masks_on_image_layers() {
    let mut l = solid("s", 100, 100, RED, 100, 100);
    l.masks = vec![Mask::new(BezPath::rect(0.0, 0.0, 50.0, 100.0))];
    let out = render(&project(vec![comp("c", 100, 100, vec![l.clone()])]), "c");
    assert_px(&out, 25, 50, [1.0, 0.0, 0.0, 1.0], 1e-5);
    assert_px(&out, 75, 50, [0.0; 4], 1e-5);
    assert!((alpha_sum(&out) - 5000.0).abs() < 1.0);

    // レイヤーを回転・拡大してもマスクはレイヤーに付いて動く
    l.transform.rotation = Property::constant(90.0);
    let out = render(&project(vec![comp("c", 100, 100, vec![l.clone()])]), "c");
    assert_px(&out, 50, 25, [1.0, 0.0, 0.0, 1.0], 1e-5);
    assert_px(&out, 50, 75, [0.0; 4], 1e-5);

    // 楕円マスク＋フェザー。1/2 解像度でも見た目（面積比）が変わらない
    l.transform.rotation = Property::constant(0.0);
    let mut m = Mask::new(BezPath::ellipse([50.0, 50.0], [30.0, 30.0]));
    m.feather = Property::constant([12.0, 12.0]);
    l.masks = vec![m];
    let p = project(vec![comp("c", 100, 100, vec![l])]);
    let full = render(&p, "c");
    let half = render_at(&p, "c", 0.0, RenderOptions { downsample: 2, ..ALPHA });
    assert!((alpha_sum(&full) / 4.0 - alpha_sum(&half)).abs() / alpha_sum(&half) < 0.02);
    let edge = full.pixel(80, 50)[3];
    assert!(edge > 0.3 && edge < 0.7, "{edge}");
}

#[test]
fn k2_masks_on_shape_layers_and_path_keyframes() {
    let mut l = shape("s", vec![ShapeItem::rect(100.0, 100.0), ShapeItem::fill([1.0; 4])], 100, 100);
    // マスクパスをキーフレームで動かす（右半分 → 左半分）。レイヤー座標は中心原点
    let a = BezPath::rect(0.0, -50.0, 50.0, 100.0);
    let b = BezPath::rect(-50.0, -50.0, 50.0, 100.0);
    let mut m = Mask::new(a.clone());
    m.path = Property::constant(a.clone()).with_keys(vec![Keyframe::linear(Time::ZERO, a), Keyframe::linear(Time::from_seconds(1.0), b)]);
    l.masks = vec![m];
    let p = project(vec![comp("c", 100, 100, vec![l])]);
    assert_px(&render_at(&p, "c", 0.0, ALPHA), 75, 50, [1.0; 4], 1e-5);
    assert_px(&render_at(&p, "c", 0.0, ALPHA), 25, 50, [0.0; 4], 1e-5);
    assert_px(&render_at(&p, "c", 1.0, ALPHA), 25, 50, [1.0; 4], 1e-5);
    // 中間時刻では x 25..75 が見える
    let mid = render_at(&p, "c", 0.5, ALPHA);
    assert_px(&mid, 50, 50, [1.0; 4], 1e-5);
    assert_px(&mid, 10, 50, [0.0; 4], 1e-5);
    assert_px(&mid, 90, 50, [0.0; 4], 1e-5);
}

fn have_fonts() -> bool {
    !znd_text::layout(&TextDocument::new("A", 50.0), None, &|_| 0.0, Time::ZERO).glyphs.is_empty()
}

#[test]
fn t1_text_layer_renders_centered_and_animates() {
    if !have_fonts() {
        eprintln!("フォントが無いためスキップ");
        return;
    }
    let mut doc = TextDocument::new("音ハメ", 60.0);
    doc.justify = Justify::Center;
    doc.fill = Some([1.0, 1.0, 0.0, 1.0]);
    let mut l = text("t", doc, 400, 200);
    let p = project(vec![comp("c", 400, 200, vec![l.clone()])]);
    let out = render(&p, "c");
    assert!(alpha_sum(&out) > 1500.0);
    // インクの左右端の中点がコンポ中央に来る（中央揃え）。ベースラインはレイヤー原点の高さ
    let xs: Vec<usize> = (0..400).filter(|x| (0..200).any(|y| out.pixel(*x, y)[3] > 0.5)).collect();
    let mid = (xs[0] + xs[xs.len() - 1]) as f64 / 2.0;
    assert!((mid - 200.0).abs() < 6.0, "mid {mid}");
    let (_, cy) = centroid(&out);
    assert!(cy < 100.0 && cy > 60.0, "cy {cy}");
    let ink = out.data.chunks_exact(4).find(|p| p[3] == 1.0).unwrap();
    assert_eq!(ink, &[1.0, 1.0, 0.0, 1.0]);

    // 位置アニメーター＋範囲セレクターのオフセットをキーフレームで流す（文字が順に落ちてくる）
    let sel = match TextSelector::range() {
        TextSelector::Range { end, amount, unit, shape, mode, randomize, seed, .. } => TextSelector::Range {
            start: Property::constant(0.0),
            end,
            offset: Property::constant(0.0)
                .with_keys(vec![Keyframe::linear(Time::ZERO, 0.0), Keyframe::linear(Time::from_seconds(1.0), 100.0)]),
            amount,
            unit,
            shape,
            mode,
            randomize,
            seed,
        },
        _ => unreachable!(),
    };
    if let LayerKind::Text { animators, .. } = &mut l.kind {
        animators.push(TextAnimator {
            name: "drop".into(),
            selectors: vec![sel],
            props: AnimatorProps {
                position: Some(Property::constant([0.0, -50.0])),
                opacity: Some(Property::constant(0.0)),
                ..Default::default()
            },
        });
    }
    let p = project(vec![comp("c", 400, 200, vec![l])]);
    // t=0: 全文字が選択されて透明。t=1: 選択範囲が抜けきって全文字が表示
    assert!(alpha_sum(&render_at(&p, "c", 0.0, ALPHA)) < 1.0);
    let mid = alpha_sum(&render_at(&p, "c", 0.5, ALPHA));
    let end = alpha_sum(&render_at(&p, "c", 1.0, ALPHA));
    assert!((end - alpha_sum(&out)).abs() < 1.0);
    assert!(mid > end * 0.2 && mid < end * 0.8, "mid {mid} end {end}");
}

#[test]
fn t2_text_on_mask_path() {
    if !have_fonts() {
        return;
    }
    let mut l = text("t", TextDocument::new("AAAAAAAA", 30.0), 300, 300);
    // 円のマスクをパスとして使う（マスクとしては無効にする）
    let mut m = Mask::new(BezPath::ellipse([0.0, 0.0], [100.0, 100.0]));
    m.mode = MaskMode::None;
    l.masks = vec![m];
    if let LayerKind::Text { path, .. } = &mut l.kind {
        *path = Some(TextPath { mask: 0, reverse: false, perpendicular: true, first_margin: Property::constant(0.0) });
    }
    let out = render(&project(vec![comp("c", 300, 300, vec![l])]), "c");
    assert!(alpha_sum(&out) > 500.0);
    // すべてのインクは半径 100 の円の外側付近（文字は円の外に立つ）にある
    for y in 0..300 {
        for x in 0..300 {
            if out.pixel(x, y)[3] > 0.5 {
                let r = ((x as f64 - 150.0).powi(2) + (y as f64 - 150.0).powi(2)).sqrt();
                assert!(r > 95.0 && r < 135.0, "ink at r={r}");
            }
        }
    }
}

/// フレーム番号 n のとき、表示値 n/10 のグレーを返す合成フッテージ。
struct Ramp;

impl FootageSource for Ramp {
    fn frame(&self, item: &FootageItem, seconds: f64) -> Result<Option<Arc<FrameBuf>>, String> {
        let n = (seconds * item.frame_rate.unwrap().fps() + 1e-6).floor();
        let mut b = FrameBuf::new(item.width as usize, item.height as usize);
        b.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[n as f32 / 10.0, n as f32 / 10.0, n as f32 / 10.0, 1.0]));
        Ok(Some(Arc::new(b)))
    }
}

fn footage_project() -> Project {
    let mut p = Project::new("f");
    p.footage.push(FootageItem {
        id: FootageId::new("f"),
        name: "f".into(),
        path: "f.mov".into(),
        width: 8,
        height: 8,
        frame_rate: Some(FrameRate { num: 10, den: 1 }),
        duration: Some(Time::from_seconds(1.0)),
        rights: Rights::default(),
        audio: None,
        vfr: false,
    });
    let l = Layer::new("l", "l", LayerKind::Footage { footage: FootageId::new("f") }, Transform::centered(8.0, 8.0, 8.0, 8.0), dur());
    p.compositions.push(comp("c", 8, 8, vec![l]));
    p
}

fn value(p: &Project, seconds: f64) -> f32 {
    Renderer::new(p, &Ramp).render_frame(&CompId::new("c"), Time::from_seconds(seconds), ALPHA).unwrap().pixel(4, 4)[0]
}

#[test]
fn m1_speed_time_remap_hold_and_frame_blend() {
    let mut p = footage_project();
    // 等速: 0.35 秒 → フレーム 3
    assert!((value(&p, 0.35) - 0.3).abs() < 1e-6);
    // 半速: 0.5 秒 → ソース 0.25 秒 → フレーム 2
    p.compositions[0].layers[0].speed = 0.5;
    assert!((value(&p, 0.5) - 0.2).abs() < 1e-6);
    // フレームブレンド: ソース 0.25 秒はフレーム 2 と 3 の中間
    p.compositions[0].layers[0].frame_blend = true;
    assert!((value(&p, 0.5) - 0.25).abs() < 1e-4);
    p.compositions[0].layers[0].frame_blend = false;
    p.compositions[0].layers[0].speed = 1.0;

    // タイムリマップ（逆再生）: コンポ 0 秒 → ソース 0.9 秒、コンポ 0.9 秒 → ソース 0 秒
    p.compositions[0].layers[0].time_remap =
        Some(Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 0.9), Keyframe::linear(Time::from_seconds(0.9), 0.0)]));
    assert!((value(&p, 0.0) - 0.9).abs() < 1e-6);
    assert!((value(&p, 0.6) - 0.3).abs() < 1e-6);
    p.compositions[0].layers[0].time_remap = None;

    // 3 コマ打ち（コンポは 30fps）: フレーム 0,1,2 は同じ絵、3 で進む
    p.compositions[0].layers[0].hold_frames = 9;
    let at = |f: i64| value(&p, f as f64 / 30.0 + 1e-4);
    assert_eq!(at(0), at(8));
    assert!((at(9) - 0.3).abs() < 1e-6);
    assert_eq!(at(9), at(17));
}

#[test]
fn a1_adjustment_layer_affects_only_layers_below_within_its_mask() {
    let mut adj = Layer::new("adj", "adj", LayerKind::Adjustment, Transform::centered(100.0, 100.0, 100.0, 100.0), dur());
    adj.effects = vec![fx("invert", 100, 100)];
    // レイヤー座標で左半分だけに効かせる（調整レイヤーはコンポと同じ大きさ）
    adj.masks = vec![Mask::new(BezPath::rect(0.0, 0.0, 50.0, 100.0))];
    let top = solid("top", 20, 20, GREEN, 100, 100);
    let p = project(vec![comp("c", 100, 100, vec![top, adj, solid("bg", 100, 100, RED, 100, 100)])]);
    let out = render(&p, "c");
    // 左: 赤が反転してシアン。右: 赤のまま。上の緑は影響を受けない
    assert_px(&out, 20, 20, [0.0, 1.0, 1.0, 1.0], 1e-4);
    assert_px(&out, 80, 20, [1.0, 0.0, 0.0, 1.0], 1e-4);
    assert_px(&out, 45, 50, [0.0, 1.0, 0.0, 1.0], 1e-4);
    assert_px(&out, 55, 50, [0.0, 1.0, 0.0, 1.0], 1e-4);
}
