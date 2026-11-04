//! GPU 合成の検証: 同じコンポを CPU と GPU で合成し、画素ごとに比べる。GPU が使えない環境ではスキップする。
//!
//! GPU に載せるレイヤー（平面・プリコンポ・GPU 対応エフェクト付き）と、CPU でタイルにしてから重ねるレイヤー
//! （マスク・トラックマット・シェイプ・モーションブラー・GPU 非対応エフェクト）を混ぜ、描画モード 31 種、
//! 回転・縮小（スーパーサンプル）・半透明、表示ガンマ空間での合成、8bpc、1/2 解像度を確かめる。

use std::sync::OnceLock;
use znd_core::shape::{BezPath, Mask, ShapeItem};
use znd_core::*;
use znd_gpu::GpuBackend;
use znd_render::{FrameBuf, NoFootage, RenderOptions, Renderer};

fn gpu() -> Option<&'static GpuBackend> {
    static GPU: OnceLock<Option<GpuBackend>> = OnceLock::new();
    GPU.get_or_init(|| match GpuBackend::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("GPU が使えないためスキップ: {e}");
            None
        }
    })
    .as_ref()
}

/// 決定的な擬似乱数（0..1）。
fn r(i: usize, k: u64) -> f64 {
    ((i as u64 * 2654435761 + k * 40503 + 17) % 10007) as f64 / 10007.0
}

fn solid(id: &str, i: usize, w: u32, h: u32, dur: Time) -> Layer {
    let color = [r(i, 1) as f32 * 1.2, r(i, 2) as f32, r(i, 3) as f32];
    let mut l = Layer::new(id, id, LayerKind::Solid { width: w, height: h, color }, Transform::centered(w as f64, h as f64, 640.0, 360.0), dur);
    l.transform.position = Property::spatial([r(i, 4) * 640.0, r(i, 5) * 360.0]);
    l.transform.rotation = Property::constant(r(i, 6) * 360.0);
    // 縮小（スーパーサンプル）と拡大を混ぜる
    let s = [15.0, 60.0, 100.0, 140.0][i % 4];
    l.transform.scale = Property::constant([s, s * (0.7 + r(i, 7) * 0.6)]);
    l.transform.opacity = Property::constant(35.0 + r(i, 8) * 65.0);
    l.blend_mode = BlendMode::ALL[i % BlendMode::ALL.len()];
    l
}

/// 平面にグラデーションを付けた「画像」レイヤーの代わりに、中身のあるプリコンポを使う。
fn project(linear: bool, depth: BitDepth, fallbacks: bool) -> Project {
    let dur = Time::from_seconds(2.0);
    let fr = FrameRate::FPS_30;
    let mut p = Project::new("parity");
    p.settings.linear_blending = linear;
    p.settings.bit_depth = depth;

    // 入れ子のコンポ（画像のソースになる）
    let mut inner = Composition::new("inner", "inner", 200, 120, fr, dur);
    for i in 0..4 {
        let mut l = solid(&format!("in{i}"), i + 50, 90, 60, dur);
        l.transform.position = Property::spatial([40.0 + i as f64 * 40.0, 60.0]);
        l.blend_mode = BlendMode::Normal;
        inner.layers.push(l);
    }
    p.compositions.push(inner);

    let mut c = Composition::new("main", "main", 640, 360, fr, dur);
    c.background = [0.0, 0.0, 0.0];
    for i in 0..40 {
        let mut l = solid(&format!("s{i}"), i, 60 + (r(i, 9) * 200.0) as u32, 40 + (r(i, 10) * 160.0) as u32, dur);
        match i % 7 {
            // GPU 対応のエフェクト（画像として送り、GPU で適用）
            1 => l.effects.extend(EffectInstance::new("gaussian_blur", 100.0, 100.0).map(|e| e.set_f("radius", 6.0))),
            2 => l.effects.extend(EffectInstance::new("levels", 100.0, 100.0).map(|e| e.set_f("gamma", 1.4))),
            3 => {
                // プリコンポ（入れ子も GPU で合成される）
                l.kind = LayerKind::Precomp { comp: CompId::new("inner") };
                l.transform.anchor = Property::constant([100.0, 60.0]);
            }
            _ => {}
        }
        if fallbacks {
            match i % 9 {
                4 => l.masks.push(Mask::new(BezPath::ellipse([50.0, 40.0], [40.0, 30.0]))),
                5 => l.effects.extend(EffectInstance::new("fractal_noise", 100.0, 100.0)),
                6 => {
                    l.kind = LayerKind::Shape { items: vec![ShapeItem::ellipse(120.0, 80.0), ShapeItem::fill([0.9, 0.4, 0.1, 1.0])] };
                }
                7 if i > 0 => l.track_matte = Some(TrackMatte { layer: LayerId::new(format!("s{}", i - 1)), mode: MatteMode::Luma }),
                8 => {
                    l.motion_blur = true;
                    l.transform.position = Property::spatial([0.0, 0.0]).with_keys(vec![
                        Keyframe::linear(Time::ZERO, [100.0, 100.0]),
                        Keyframe::linear(dur, [500.0, 260.0]),
                    ]);
                }
                _ => {}
            }
        }
        c.layers.push(l);
    }
    if fallbacks {
        c.motion_blur = MotionBlur { enabled: true, shutter_angle: 180.0, shutter_phase: -90.0, samples: 8 };
    }
    p.compositions.push(c);
    p
}

/// 差の最大値と、その画素。`display` なら表示値（sRGB）で比べる（8bpc の量子化は表示値の段で行われるため）。
fn max_diff(a: &FrameBuf, b: &FrameBuf, display: bool) -> (f32, usize) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let to_display = |v: f32| if display { znd_render::color::linear_to_srgb(v.max(0.0)) } else { v };
    let mut worst = (0.0f32, 0usize);
    for (i, (x, y)) in a.data.iter().zip(&b.data).enumerate() {
        let (x, y) = (to_display(*x), to_display(*y));
        let d = (x - y).abs() / x.abs().max(1.0);
        if d > worst.0 {
            worst = (d, i / 4);
        }
    }
    worst
}

fn check(name: &str, p: &Project, downsample: u32, tolerance: f32) {
    let Some(g) = gpu() else { return };
    let opts = RenderOptions { downsample, ..RenderOptions::PREVIEW };
    let id = CompId::new("main");
    let cpu = Renderer::new(p, &NoFootage);
    let gpu = Renderer::new(p, &NoFootage).with_composite(Some(g));
    for frame in [0, 17, 45] {
        let t = FrameRate::FPS_30.frame_to_time(frame);
        let a = cpu.render_frame(&id, t, opts).unwrap();
        let b = gpu.render_frame(&id, t, opts).unwrap();
        let (d, at) = max_diff(&a, &b, p.settings.bit_depth == BitDepth::B8);
        let (x, y) = (at % a.width, at / a.width);
        assert!(d <= tolerance, "{name} フレーム {frame}: 最大の差 {d:.5}（({x}, {y}) CPU {:?} / GPU {:?}）", a.pixel(x, y), b.pixel(x, y));
    }
}

#[test]
fn every_blend_mode_and_transform_matches_cpu() {
    check("リニア", &project(true, BitDepth::F32, false), 1, 0.002);
}

#[test]
fn gamma_space_blending_matches_cpu() {
    check("表示ガンマ", &project(false, BitDepth::F32, false), 1, 0.002);
}

#[test]
fn eight_bit_matches_cpu() {
    // 8bpc は最後に表示値の 1/255 段へ丸めるので、丸めの境目で 1 段ずれることがある（表示値で比べる）
    check("8bpc", &project(true, BitDepth::B8, false), 1, 1.0 / 255.0 + 1e-3);
}

#[test]
fn cpu_fallback_layers_are_blended_on_gpu() {
    check("CPU のタイル混在", &project(true, BitDepth::F32, true), 1, 0.002);
}

#[test]
fn half_resolution_matches_cpu() {
    check("1/2 解像度", &project(true, BitDepth::F32, true), 2, 0.002);
}

#[test]
fn stencil_clears_outside_the_layer() {
    let Some(_) = gpu() else { return };
    let mut p = project(true, BitDepth::F32, false);
    let c = p.compositions.iter_mut().find(|c| c.id.0 == "main").unwrap();
    c.layers[0].blend_mode = BlendMode::StencilAlpha;
    c.layers[3].blend_mode = BlendMode::StencilLuma;
    check("ステンシル", &p, 1, 0.002);
}
