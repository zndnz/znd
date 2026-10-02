//! M1 受け入れテスト（docs/05-acceptance-criteria.md の E 系列）: H.264 書き出しとフッテージ読み込み。
//! ffmpeg が無い環境ではスキップする。

use std::path::PathBuf;
use znd_core::demo::demo_project;
use znd_core::*;
use znd_media::probe::probe_with;
use znd_media::*;
use znd_render::{to_rgb8, FrameBuf, NoFootage, RenderOptions, Renderer};

fn have_ffmpeg() -> bool {
    let ok = tools::ffmpeg().is_ok() && tools::ffprobe().is_ok();
    if !ok {
        eprintln!("ffmpeg/ffprobe が無いためスキップ");
    }
    ok
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("znd-media-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// デモを 1 秒（30 フレーム）に切り詰めたもの。
fn short_demo() -> Project {
    let mut p = demo_project();
    for c in &mut p.compositions {
        c.duration = Time::from_seconds(1.0);
    }
    p
}

fn fast() -> H264Settings {
    H264Settings { crf: 14, preset: "veryfast".into() }
}

fn main_id() -> CompId {
    CompId::new("main")
}

/// 8bit sRGB での PSNR (dB)。
fn psnr(a: &FrameBuf, b: &FrameBuf) -> f64 {
    let (a, b) = (to_rgb8(a, None), to_rgb8(b, None));
    let mse = a.iter().zip(&b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
}

/// 書き出した mp4 を 1 枚のフッテージレイヤーとして置いたプロジェクト。
fn project_with_footage(path: &std::path::Path) -> Project {
    let mut p = Project::new("roundtrip");
    let id = import_footage(&mut p, path).unwrap();
    let item = p.footage_item(&id).unwrap().clone();
    let dur = item.duration.unwrap_or(Time::from_seconds(1.0));
    let layer = Layer::new(
        "f",
        "footage",
        LayerKind::Footage { footage: id },
        Transform::centered(item.width as f64, item.height as f64, item.width as f64, item.height as f64),
        dur,
    );
    p.compositions.push(Composition {
        id: main_id(),
        name: "main".into(),
        width: item.width,
        height: item.height,
        pixel_aspect: 1.0,
        frame_rate: item.frame_rate.unwrap_or(FrameRate::FPS_30),
        duration: dur,
        background: [0.0; 3],
        layers: vec![layer],
        markers: vec![],
        ..Composition::blank()
    });
    p
}

#[test]
fn e1_h264_export_has_expected_stream_properties() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("e1");
    let out = dir.join("demo.mp4");
    let p = short_demo();
    let mut calls = Vec::new();
    export_h264(&p, &main_id(), &NoFootage, &out, &fast(), |done, total| {
        calls.push((done, total));
        true
    })
    .unwrap();
    assert_eq!(calls.last(), Some(&(30, 30)));

    let info = probe_with(&out, true).unwrap();
    assert_eq!(info.codec, "h264");
    assert_eq!((info.width, info.height), (1920, 1080));
    assert_eq!(info.frame_rate, Some(FrameRate::FPS_30));
    assert_eq!(info.frame_count, Some(30));
    assert_eq!(info.duration, Some(Time::from_seconds(1.0)));
    assert!(!info.maybe_vfr);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn e2_exported_video_roundtrips_frame_accurately() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("e2");
    let out = dir.join("demo.mp4");
    let p = short_demo();
    export_h264(&p, &main_id(), &NoFootage, &out, &fast(), |_, _| true).unwrap();

    let reference = |frame: i64| {
        Renderer::new(&p, &NoFootage).render_frame(&main_id(), FrameRate::FPS_30.frame_to_time(frame), RenderOptions::EXPORT).unwrap()
    };
    let rp = project_with_footage(&out);
    let footage = FfmpegFootage::new(&dir);
    let decoded = |frame: i64| {
        Renderer::new(&rp, &footage).render_frame(&main_id(), FrameRate::FPS_30.frame_to_time(frame), RenderOptions::EXPORT).unwrap()
    };

    // ランダムアクセス（後ろ→前→連続）しても、常に同じ番号のフレームが返ること
    for frame in [20i64, 5, 6, 29, 0] {
        let got = decoded(frame);
        let same = psnr(&got, &reference(frame));
        assert!(same > 38.0, "frame {frame}: PSNR {same:.1} dB");
        for neighbour in [frame - 1, frame + 1] {
            if (0..30).contains(&neighbour) {
                let other = psnr(&got, &reference(neighbour));
                assert!(same > other + 3.0, "frame {frame} が {neighbour} と区別できない（{same:.1} vs {other:.1} dB）");
            }
        }
    }
    // 尺の外は透明（背景色のみ）
    let past = Renderer::new(&rp, &footage)
        .render_frame(&main_id(), Time::from_seconds(5.0), RenderOptions { background: false, ..RenderOptions::EXPORT })
        .unwrap();
    assert!(past.data.iter().all(|v| *v == 0.0));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn e3_png_still_footage_is_lossless() {
    let dir = temp_dir("e3");
    let p = demo_project();
    let frame = Renderer::new(&p, &NoFootage)
        .render_frame(&main_id(), Time::from_seconds(1.5), RenderOptions { downsample: 4, ..RenderOptions::EXPORT })
        .unwrap();
    let png = dir.join("still.png");
    save_png(&frame, &png, None).unwrap();

    let rp = project_with_footage(&png);
    assert_eq!(rp.footage[0].frame_rate, None);
    let footage = FfmpegFootage::new(&dir);
    let back = Renderer::new(&rp, &footage).render_frame(&main_id(), Time::ZERO, RenderOptions::EXPORT).unwrap();
    assert_eq!(to_rgb8(&back, None), to_rgb8(&frame, None));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn e4_odd_resolution_and_cancel_are_handled() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("e4");
    let out = dir.join("x.mp4");
    let mut odd = short_demo();
    odd.compositions[0].width = 1919;
    let err = export_h264(&odd, &main_id(), &NoFootage, &out, &fast(), |_, _| true).unwrap_err();
    assert!(err.to_string().contains("偶数"), "{err}");

    let err = export_h264(&short_demo(), &main_id(), &NoFootage, &out, &fast(), |_, _| false).unwrap_err();
    assert!(matches!(err, ExportError::Cancelled));
    assert!(!out.exists(), "中断時に書きかけのファイルが残っている");
    std::fs::remove_dir_all(dir).unwrap();
}
