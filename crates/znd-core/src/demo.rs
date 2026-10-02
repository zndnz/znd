//! 外部素材なしで M1 の機能を一通り通すデモプロジェクト。受け入れテストの基準コンポも兼ねる。

use crate::model::*;
use crate::property::{Keyframe, Property};
use crate::time::{FrameRate, Time};

fn s(sec: f64) -> Time {
    Time::from_seconds(sec)
}

fn solid(id: &str, name: &str, w: u32, h: u32, color: [f32; 3], comp: (u32, u32), dur: Time) -> Layer {
    Layer::new(
        id,
        name,
        LayerKind::Solid { width: w, height: h, color },
        Transform::centered(w as f64, h as f64, comp.0 as f64, comp.1 as f64),
        dur,
    )
}

pub fn demo_project() -> Project {
    let dur = s(5.0);
    let main = (1920u32, 1080u32);
    let bars = (1200u32, 600u32);

    // --- プリコンポ: 伸縮するカラーバー ---
    let palette: [[f32; 3]; 6] =
        [[1.0, 0.25, 0.3], [1.0, 0.65, 0.1], [0.95, 0.9, 0.2], [0.2, 0.85, 0.5], [0.2, 0.6, 1.0], [0.7, 0.35, 1.0]];
    let mut bar_layers = Vec::new();
    for (i, color) in palette.iter().enumerate() {
        let mut l = solid(&format!("bar-{i}"), &format!("バー {}", i + 1), 160, 600, *color, bars, dur);
        l.transform.anchor = Property::constant([80.0, 600.0]);
        l.transform.position = Property::spatial([100.0 + 200.0 * i as f64, 600.0]);
        let phase = i as f64 * 0.15;
        l.transform.scale = Property::constant([100.0, 100.0]).with_keys(vec![
            Keyframe::easy_ease(s(phase), [100.0, 20.0]),
            Keyframe::easy_ease(s(phase + 0.5), [100.0, 100.0]),
            Keyframe::easy_ease(s(phase + 1.0), [100.0, 35.0]),
            Keyframe::easy_ease(s(phase + 1.5), [100.0, 80.0]),
            Keyframe::easy_ease(s(phase + 2.5), [100.0, 20.0]),
            Keyframe::easy_ease(s(phase + 3.5), [100.0, 100.0]),
        ]);
        bar_layers.push(l);
    }
    let bars_comp = Composition {
        id: CompId::new("bars"),
        name: "カラーバー".into(),
        width: bars.0,
        height: bars.1,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        duration: dur,
        background: [0.0; 3],
        layers: bar_layers,
        markers: Vec::new(),
        ..Composition::blank()
    };

    // --- メインコンポ ---
    let mut flash = solid("flash", "フラッシュ", 1920, 1080, [1.0, 1.0, 1.0], main, dur);
    flash.blend_mode = BlendMode::Add;
    flash.transform.opacity = Property::constant(0.0).with_keys(
        (0..5)
            .flat_map(|beat| {
                let t = beat as f64;
                [Keyframe::linear(s(t), 35.0), Keyframe::easy_ease(s(t + 0.4), 0.0), Keyframe::hold(s(t + 0.9), 0.0)]
            })
            .collect(),
    );

    let mut matte = solid("matte", "マット（回転バー）", 1400, 260, [1.0, 1.0, 1.0], main, dur);
    matte.visible = false;
    matte.transform.rotation = Property::constant(0.0).with_keys(vec![Keyframe::linear(s(0.0), -20.0), Keyframe::linear(s(5.0), 160.0)]);

    let mut bars_layer = Layer::new(
        "bars-layer",
        "カラーバー",
        LayerKind::Precomp { comp: CompId::new("bars") },
        Transform::centered(bars.0 as f64, bars.1 as f64, main.0 as f64, main.1 as f64),
        dur,
    );
    bars_layer.track_matte = Some(TrackMatte { layer: LayerId::new("matte"), mode: MatteMode::Alpha });

    let mut orbit = Layer::new("orbit", "軌道ヌル", LayerKind::Null, Transform::centered(0.0, 0.0, 1920.0, 1080.0), dur);
    orbit.transform.rotation = Property::constant(0.0).with_keys(vec![Keyframe::linear(s(0.0), 0.0), Keyframe::linear(s(5.0), 360.0)]);

    let mut sat_a = solid("sat-a", "衛星 A", 160, 160, [1.0, 0.55, 0.1], main, dur);
    sat_a.parent = Some(LayerId::new("orbit"));
    sat_a.transform.position = Property::spatial([420.0, 0.0]);
    sat_a.blend_mode = BlendMode::Screen;

    let mut sat_b = solid("sat-b", "衛星 B", 160, 160, [0.1, 0.8, 1.0], main, dur);
    sat_b.parent = Some(LayerId::new("orbit"));
    sat_b.transform.position = Property::spatial([-420.0, 0.0]);
    sat_b.transform.rotation = Property::constant(45.0);
    sat_b.blend_mode = BlendMode::Add;

    let mut mover = solid("mover", "ムーバー", 300, 300, [0.95, 0.2, 0.7], main, dur);
    mover.transform.position = Property::spatial([960.0, 540.0]).with_keys(vec![
        Keyframe::easy_ease(s(0.0), [300.0, 800.0]).with_auto_path(),
        Keyframe::easy_ease(s(1.5), [960.0, 250.0]).with_auto_path(),
        Keyframe::easy_ease(s(3.0), [1620.0, 800.0]).with_auto_path(),
        Keyframe::easy_ease(s(4.5), [960.0, 540.0]).with_auto_path(),
    ]);
    mover.transform.rotation = Property::constant(0.0).with_keys(vec![
        Keyframe::auto_bezier(s(0.0), 0.0),
        Keyframe::auto_bezier(s(3.0), 180.0),
        Keyframe::auto_bezier(s(4.5), 225.0),
    ]);
    mover.transform.scale = Property::constant([100.0, 100.0]).with_keys(vec![
        Keyframe::easy_ease(s(0.0), [60.0, 60.0]),
        Keyframe::easy_ease(s(1.5), [120.0, 120.0]),
        Keyframe::easy_ease(s(4.5), [80.0, 80.0]),
    ]);
    mover.transform.opacity = Property::constant(85.0);

    let mut glass = solid("glass", "オーバーレイ板", 900, 900, [0.25, 0.4, 0.9], main, dur);
    glass.blend_mode = BlendMode::Overlay;
    glass.transform.rotation = Property::constant(0.0).with_keys(vec![Keyframe::linear(s(0.0), 45.0), Keyframe::linear(s(5.0), 0.0)]);

    let bg = solid("bg", "背景", 1920, 1080, [0.05, 0.06, 0.12], main, dur);

    let main_comp = Composition {
        id: CompId::new("main"),
        name: "メイン".into(),
        width: main.0,
        height: main.1,
        pixel_aspect: 1.0,
        frame_rate: FrameRate::FPS_30,
        duration: dur,
        background: [0.0; 3],
        layers: vec![flash, matte, bars_layer, orbit, sat_a, sat_b, mover, glass, bg],
        markers: (0..5).map(|b| Marker { time: s(b as f64), duration: Time::ZERO, comment: format!("拍 {}", b + 1) }).collect(),
        ..Composition::blank()
    };

    let mut project = Project::new("デモ");
    project.compositions = vec![main_comp, bars_comp];
    project
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_is_valid() {
        assert_eq!(super::demo_project().validate(), Vec::<String>::new());
    }
}
