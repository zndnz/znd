//! M2/M3 受け入れテスト（入出力側）: 音声の多重化、各出力形式、VFR、プロキシ。
//! ffmpeg が無い環境ではスキップする。

use std::path::{Path, PathBuf};
use std::process::Command;
use znd_audio::{Clip, ClipSource};
use znd_core::*;
use znd_media::probe::probe_with;
use znd_media::*;
use znd_render::{to_rgb8, FootageSource, FrameBuf, NoFootage, RenderOptions, Renderer};

fn have_ffmpeg() -> bool {
    let ok = tools::ffmpeg().is_ok() && tools::ffprobe().is_ok();
    if !ok {
        eprintln!("ffmpeg/ffprobe が無いためスキップ");
    }
    ok
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("znd-formats-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const W: u32 = 128;
const H: u32 = 72;

fn main_id() -> CompId {
    CompId::new("main")
}

/// フレームごとに絵が変わる小さなコンポ（白い四角が左から右へ動き、背景の明るさも変わる）。
fn moving_project(seconds: f64) -> Project {
    let dur = Time::from_seconds(seconds);
    let mut sq = Layer::new(
        "sq",
        "sq",
        LayerKind::Solid { width: 16, height: 40, color: [1.0, 1.0, 1.0] },
        Transform::centered(16.0, 40.0, W as f64, H as f64),
        dur,
    );
    sq.transform.position =
        Property::spatial([0.0, 36.0]).with_keys(vec![Keyframe::linear(Time::ZERO, [10.0, 36.0]), Keyframe::linear(dur, [118.0, 36.0])]);
    let mut bg = Layer::new(
        "bg",
        "bg",
        LayerKind::Solid { width: W, height: H, color: [0.2, 0.4, 0.9] },
        Transform::centered(W as f64, H as f64, W as f64, H as f64),
        dur,
    );
    bg.transform.opacity = Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 20.0), Keyframe::linear(dur, 100.0)]);
    let mut p = Project::new("moving");
    p.compositions.push(Composition { layers: vec![sq, bg], ..Composition::new("main", "main", W, H, FrameRate::FPS_30, dur) });
    p
}

fn reference(p: &Project, frame: i64) -> FrameBuf {
    Renderer::new(p, &NoFootage).render_frame(&main_id(), FrameRate::FPS_30.frame_to_time(frame), RenderOptions::EXPORT).unwrap()
}

fn psnr(a: &FrameBuf, b: &FrameBuf) -> f64 {
    let (a, b) = (to_rgb8(a, None), to_rgb8(b, None));
    let mse = a.iter().zip(&b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
}

/// デコードした絵が、元のコンポの何フレーム目に最も近いか。
fn identify(p: &Project, got: &FrameBuf, frames: i64) -> i64 {
    (0..frames).max_by(|a, b| psnr(got, &reference(p, *a)).total_cmp(&psnr(got, &reference(p, *b)))).unwrap()
}

/// 書き出したファイルを 1 枚のフッテージレイヤーにしたプロジェクト。
fn project_with_footage(path: &Path) -> (Project, Vec<String>) {
    let mut p = Project::new("roundtrip");
    let (id, warnings) = import_footage_with_warnings(&mut p, path).unwrap();
    let item = p.footage_item(&id).unwrap().clone();
    let dur = item.duration.unwrap_or(Time::from_seconds(1.0));
    let layer = Layer::new(
        "f",
        "f",
        LayerKind::Footage { footage: id },
        Transform::centered(item.width as f64, item.height as f64, item.width as f64, item.height as f64),
        dur,
    );
    p.compositions.push(Composition {
        layers: vec![layer],
        ..Composition::new("main", "main", item.width, item.height, item.frame_rate.unwrap_or(FrameRate::FPS_30), dur)
    });
    (p, warnings)
}

fn decoded(p: &Project, footage: &dyn FootageSource, frame: i64, background: bool) -> FrameBuf {
    let fr = p.compositions[0].frame_rate;
    Renderer::new(p, footage)
        .render_frame(&main_id(), fr.frame_to_time(frame), RenderOptions { background, ..RenderOptions::EXPORT })
        .unwrap()
}

/// 既知の位置にクリックを置いた音声（1 秒ごとに振幅の違うパルス）。
fn click_track(seconds: usize) -> Vec<f32> {
    let mut s = vec![0.0f32; seconds * 48000 * 2];
    for k in 0..seconds {
        let at = k * 48000 + 4800;
        for i in 0..48 {
            let v = 0.2 + 0.1 * k as f32;
            s[(at + i) * 2] = v;
            s[(at + i) * 2 + 1] = v;
        }
    }
    s
}

fn add_audio_layer(p: &mut Project, wav: &Path, start: Time) {
    let id = import_footage(p, wav).unwrap();
    let dur = p.compositions[0].duration;
    let mut l = Layer::new("audio", "audio", LayerKind::Audio { footage: id }, Transform::centered(0.0, 0.0, 0.0, 0.0), dur);
    l.start_time = start;
    l.in_point = start;
    p.compositions[0].layers.push(l);
}

#[test]
fn a1_wav_roundtrip_and_audio_only_probe() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("a1");
    let wav = dir.join("clicks.wav");
    let src = click_track(3);
    write_wav(&wav, &src).unwrap();
    let info = probe(&wav).unwrap();
    assert_eq!((info.width, info.height, info.frame_rate), (0, 0, None));
    let a = info.audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (48000, 2));
    assert_eq!(a.duration, Time::from_seconds(3.0));
    // float WAV はビット単位で元に戻る
    assert_eq!(decode_audio(&wav).unwrap().data, src);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a4_exported_audio_stays_in_sync_sample_accurately() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("a4");
    let wav = dir.join("clicks.wav");
    write_wav(&wav, &click_track(3)).unwrap();
    let mut p = moving_project(3.0);
    // 音声を 7 フレーム（0.2333… 秒 = 11200 サンプル）遅らせて置く
    let start = FrameRate::FPS_30.frame_to_time(7);
    add_audio_layer(&mut p, &wav, start);
    let clips = FfmpegClips::new(&dir);
    let ctx = ExportContext { clips: Some(&clips), ..ExportContext::new(&p, &NoFootage) };

    // ProRes（PCM 音声）: サンプル単位で一致する
    let mov = dir.join("out.mov");
    let report = export(&ctx, &main_id(), &mov, &ExportSettings::new(Format::ProRes422), |_, _| true).unwrap();
    assert_eq!(report.frames, 90);
    let l = report.loudness.unwrap();
    assert_eq!(l.clipped_samples, 0);
    let got = decode_audio(&mov).unwrap();
    let first_click = 11200 + 4800;
    assert!(got.data[(first_click - 1) * 2].abs() < 1e-3, "クリックの 1 サンプル前は無音");
    assert!((got.data[first_click * 2] - 0.2).abs() < 1e-3, "クリックの先頭: {}", got.data[first_click * 2]);
    assert!((got.data[(first_click + 48000) * 2] - 0.3).abs() < 1e-3, "1 秒後のクリック");
    assert!(got.data[(first_click + 48) * 2].abs() < 1e-3);
    // 映像のフレーム数と音声の長さが一致する（末尾でずれない）
    let info = probe_with(&mov, true).unwrap();
    assert_eq!(info.frame_count, Some(90));
    assert_eq!(info.codec, "prores");
    assert!((got.frames() as i64 - 144000).abs() <= 1, "{}", got.frames());
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);

    // H.264 + AAC: 非可逆だが位置は保たれる（±2ms）
    let mp4 = dir.join("out.mp4");
    export(&ctx, &main_id(), &mp4, &ExportSettings::new(Format::H264 { crf: 20, preset: "veryfast".into() }), |_, _| true).unwrap();
    let got = decode_audio(&mp4).unwrap();
    let peak = (0..48000).max_by(|a, b| got.data[*a * 2].abs().total_cmp(&got.data[*b * 2].abs())).unwrap();
    assert!((peak as i64 - first_click as i64 - 24).abs() < 96 + 24, "peak at {peak}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn o1_screening_preset_pads_black_and_reports_loudness() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("o1");
    let wav = dir.join("tone.wav");
    // -20 dBFS の 1kHz
    let tone: Vec<f32> = (0..48000)
        .flat_map(|i| {
            let v = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin() * 0.1;
            [v, v]
        })
        .collect();
    write_wav(&wav, &tone).unwrap();
    let mut p = moving_project(1.0);
    add_audio_layer(&mut p, &wav, Time::ZERO);
    let clips = FfmpegClips::new(&dir);
    let ctx = ExportContext { clips: Some(&clips), ..ExportContext::new(&p, &NoFootage) };
    let mut settings = ExportSettings::screening(0.5);
    settings.format = Format::H264 { crf: 16, preset: "veryfast".into() };
    let out = dir.join("screening.mp4");
    let report = export(&ctx, &main_id(), &out, &settings, |_, _| true).unwrap();
    // 0.5 秒 + 1 秒 + 0.5 秒 = 60 フレーム
    assert_eq!(report.frames, 60);
    let lufs = report.loudness.as_ref().unwrap().integrated_lufs.unwrap();
    assert!((lufs + 20.0).abs() < 0.3, "{lufs}");
    let text = report.to_text(&p.compositions[0]);
    assert!(text.contains("統合ラウドネス: -20.") && text.contains("トゥルーピーク") && text.contains("フレーム数: 60"), "{text}");
    // 権利未確認の素材（読み込んだ音声）が警告に出る
    assert!(report.warnings.iter().any(|w| w.contains("tone.wav") && w.contains("未確認")), "{:?}", report.warnings);

    let (rp, _) = project_with_footage(&out);
    let footage = FfmpegFootage::new(&dir);
    // 冒頭と末尾は黒、本編の先頭は 15 フレーム目
    let lum = |f: i64| to_rgb8(&decoded(&rp, &footage, f, true), None).iter().map(|v| *v as f64).sum::<f64>() / (W * H * 3) as f64;
    assert!(lum(0) < 2.0 && lum(14) < 2.0 && lum(59) < 2.0 && lum(45) < 2.0, "{} {} {}", lum(0), lum(14), lum(59));
    assert!(lum(15) > 10.0 && lum(44) > 10.0);
    assert_eq!(identify(&p, &decoded(&rp, &footage, 15 + 12, true), 30), 12);
    // 音声も 0.5 秒遅れて始まる
    let audio = decode_audio(&out).unwrap();
    let energy = |a: usize, b: usize| audio.data[a * 2..b * 2].iter().map(|v| v * v).sum::<f32>();
    assert!(energy(0, 22000) < 1e-3 && energy(26000, 40000) > 1.0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn o2_h265_crf_and_two_pass() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("o2");
    let p = moving_project(1.0);
    let ctx = ExportContext::new(&p, &NoFootage);
    for (name, two_pass) in [("crf.mp4", None), ("2pass.mp4", Some(400))] {
        let out = dir.join(name);
        export(
            &ctx,
            &main_id(),
            &out,
            &ExportSettings::new(Format::H265 { crf: 18, preset: "ultrafast".into(), two_pass_kbps: two_pass }),
            |_, _| true,
        )
        .unwrap();
        let info = probe_with(&out, true).unwrap();
        assert_eq!(info.codec, "hevc", "{name}");
        assert_eq!((info.width, info.height, info.frame_count), (W, H, Some(30)), "{name}");
        let (rp, _) = project_with_footage(&out);
        let footage = FfmpegFootage::new(&dir);
        assert_eq!(identify(&p, &decoded(&rp, &footage, 20, true), 30), 20, "{name}");
    }
    // 2 パスの中間ファイル・統計ファイルは残らない
    let left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left.len(), 2, "{left:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn o3_prores4444_keeps_alpha() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("o3");
    let mut p = moving_project(0.5);
    // 背景レイヤーを消して、四角だけ（周りは透明）にする
    p.compositions[0].layers.pop();
    let out = dir.join("alpha.mov");
    let settings = ExportSettings { transparent: true, ..ExportSettings::new(Format::ProRes4444) };
    export(&ExportContext::new(&p, &NoFootage), &main_id(), &out, &settings, |_, _| true).unwrap();
    let (rp, _) = project_with_footage(&out);
    let footage = FfmpegFootage::new(&dir);
    let got = decoded(&rp, &footage, 7, false);
    let want = Renderer::new(&p, &NoFootage)
        .render_frame(&main_id(), FrameRate::FPS_30.frame_to_time(7), RenderOptions { background: false, ..RenderOptions::EXPORT })
        .unwrap();
    // アルファが保たれ、四角の位置も合っている
    let diff = got.data.chunks_exact(4).zip(want.data.chunks_exact(4)).map(|(a, b)| (a[3] - b[3]).abs()).fold(0.0f32, f32::max);
    assert!(diff < 0.02, "alpha diff {diff}");
    assert_eq!(got.pixel(2, 2)[3], 0.0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn o4_image_sequences_and_gif() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("o4");
    let p = moving_project(0.5);
    let ctx = ExportContext::new(&p, &NoFootage);
    // 範囲 5..9 の PNG 連番（16bit）: ファイル番号はコンポのフレーム番号
    let settings = ExportSettings { range: Some((5, 9)), ..ExportSettings::new(Format::PngSequence { sixteen_bit: true }) };
    let report = export(&ctx, &main_id(), &dir.join("shot.png"), &settings, |_, _| true).unwrap();
    assert_eq!(report.frames, 4);
    assert!(dir.join("shot_00005.png").exists() && dir.join("shot_00008.png").exists() && !dir.join("shot_00009.png").exists());
    let img = image::open(dir.join("shot_00006.png")).unwrap();
    assert!(matches!(img.color(), image::ColorType::Rgba16));
    // 16bit PNG を読み戻すと、8bit 相当で元のレンダーと一致する
    let (rp, _) = project_with_footage(&dir.join("shot_00006.png"));
    let footage = FfmpegFootage::new(&dir);
    assert_eq!(to_rgb8(&decoded(&rp, &footage, 0, true), None), to_rgb8(&reference(&p, 6), None));

    // EXR 連番はリニア値のまま
    export(
        &ctx,
        &main_id(),
        &dir.join("lin.exr"),
        &ExportSettings { range: Some((0, 2)), ..ExportSettings::new(Format::ExrSequence) },
        |_, _| true,
    )
    .unwrap();
    let exr = image::open(dir.join("lin_00001.exr")).unwrap().into_rgba32f();
    let want = reference(&p, 1);
    let max = exr.as_raw().iter().zip(&want.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(max < 1e-5, "exr diff {max}");

    // GIF（1/2 解像度）
    let gif = dir.join("anim.gif");
    export(&ctx, &main_id(), &gif, &ExportSettings { downsample: 2, ..ExportSettings::new(Format::Gif) }, |_, _| true).unwrap();
    let info = probe_with(&gif, true).unwrap();
    assert_eq!((info.codec.as_str(), info.width, info.height, info.frame_count), ("gif", W / 2, H / 2, Some(15)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn v1_variable_frame_rate_footage_is_conformed() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("v1");
    let p = moving_project(2.0);
    let cfr = dir.join("cfr.mp4");
    export_h264(&p, &main_id(), &NoFootage, &cfr, &H264Settings { crf: 12, preset: "veryfast".into() }, |_, _| true).unwrap();
    // 5 フレームに 1 枚（n mod 5 == 1）を落として可変フレームレートにする
    let vfr = dir.join("vfr.mkv");
    let st = Command::new(tools::ffmpeg().unwrap())
        .args(["-v", "error", "-y", "-i"])
        .arg(&cfr)
        .args(["-vf", "select='not(eq(mod(n\\,5)\\,1))'", "-fps_mode", "vfr", "-c:v", "libx264", "-crf", "12", "-preset", "veryfast"])
        .arg(&vfr)
        .status()
        .unwrap();
    assert!(st.success());
    let (rp, warnings) = project_with_footage(&vfr);
    assert!(rp.footage[0].vfr, "VFR と判定されること");
    assert!(warnings.iter().any(|w| w.contains("可変フレームレート")), "{warnings:?}");
    // 60 フレーム中 48 枚が 2 秒に並ぶ（平均 約 24fps）→ 最も近いプリセットへ適合
    let fr = rp.footage[0].frame_rate.unwrap();
    assert!(fr == FrameRate::FPS_24 || fr == FrameRate::FPS_23_976, "{fr:?}");

    // 適合後の各フレームは「その時刻に画面に出ているはずのフレーム」＝表示時刻がその時刻以前で最も新しいもの
    // （可変フレームレートでは、次のフレームが来るまで前のフレームが表示され続ける）
    let footage = FfmpegFootage::new(&dir);
    let kept: Vec<i64> = (0..60).filter(|n| n % 5 != 1).collect();
    for f in [0i64, 7, 13, 22, 30, 41] {
        let t = f as f64 / fr.fps();
        let got = identify(&p, &decoded(&rp, &footage, f, true), 60);
        // 時刻は出力フレームの刻みに丸められるので、半フレームぶんの先読みを許す
        let pos = t * 30.0 + 0.5;
        let showing = kept.iter().copied().filter(|k| *k as f64 <= pos).max().unwrap();
        assert!((got - showing).abs() <= 1, "出力 {f}（{t:.3} 秒）: 元フレーム {got}、期待 {showing}");
    }
    // ランダムアクセスしても同じ結果（シークの起点によらない）
    let a = identify(&p, &decoded(&rp, &footage, 30, true), 60);
    let _ = decoded(&rp, &footage, 3, true);
    assert_eq!(identify(&p, &decoded(&rp, &footage, 30, true), 60), a);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn p1_proxy_replaces_source_transparently() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("p1");
    let p = moving_project(1.0);
    let src = dir.join("src.mp4");
    export_h264(&p, &main_id(), &NoFootage, &src, &H264Settings { crf: 12, preset: "veryfast".into() }, |_, _| true).unwrap();
    let (rp, _) = project_with_footage(&src);
    let footage = FfmpegFootage::new(&dir);
    let full = decoded(&rp, &footage, 17, true);

    let (path, w, h) = make_proxy(&dir, &rp.footage[0], 2).unwrap();
    assert_eq!((w, h), (W / 2, H / 2));
    assert!(path.ends_with(".znd-proxy/footage-1.mp4"));
    // 新しいフッテージ供給元が、既存のプロキシを見つけて使う
    let with_proxy = FfmpegFootage::new(&dir);
    assert_eq!(with_proxy.attach_existing_proxies(&rp), 1);
    let low = decoded(&rp, &with_proxy, 17, true);
    // 出力の大きさ・位置は変わらず、絵は少し甘くなるだけ。フレーム番号もずれない
    assert_eq!((low.width, low.height), (full.width, full.height));
    assert!(psnr(&low, &full) > 24.0, "{}", psnr(&low, &full));
    assert_eq!(identify(&p, &low, 30), 17);
    // 静止画にはプロキシを作らない
    let mut still = rp.footage[0].clone();
    still.frame_rate = None;
    assert!(make_proxy(&dir, &still, 2).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn q1_render_queue_runs_all_jobs_and_reports_failures() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("q1");
    let p = moving_project(0.3);
    let job = |name: &str, comp: &str| QueueJob {
        comp: CompId::new(comp),
        path: dir.join(name),
        settings: ExportSettings::new(Format::H264 { crf: 20, preset: "ultrafast".into() }),
    };
    let jobs = vec![job("a.mp4", "main"), job("b.mp4", "存在しない"), job("c.mp4", "main")];
    // キューは JSON で保存・復元できる
    let json = serde_json::to_string(&jobs).unwrap();
    assert_eq!(serde_json::from_str::<Vec<QueueJob>>(&json).unwrap(), jobs);
    let mut events = Vec::new();
    let results = run_queue(&ExportContext::new(&p, &NoFootage), &jobs, |e| {
        if !matches!(e, QueueEvent::Progress { .. }) {
            events.push(e);
        }
        true
    });
    assert!(results[0].is_ok() && results[1].is_err() && results[2].is_ok());
    assert!(dir.join("a.mp4").exists() && !dir.join("b.mp4").exists() && dir.join("c.mp4").exists());
    assert_eq!(events.len(), 6);
    assert!(matches!(&events[3], QueueEvent::Finished { index: 1, result: Err(_) }));
    std::fs::remove_dir_all(dir).unwrap();
}

/// クリップ供給元が無い（`clips: None`）ときは映像だけが書き出される。
#[test]
fn a5_export_without_clip_source_is_video_only() {
    if !have_ffmpeg() {
        return;
    }
    struct Never;
    impl ClipSource for Never {
        fn clip(&self, _: &FootageItem) -> Result<std::sync::Arc<Clip>, String> {
            panic!("呼ばれないはず")
        }
    }
    let dir = temp_dir("a5");
    let p = moving_project(0.2);
    let out = dir.join("v.mp4");
    // 音声レイヤーが無ければ、クリップ供給元があっても音声ストリームは作られない
    let ctx = ExportContext { clips: Some(&Never), ..ExportContext::new(&p, &NoFootage) };
    let report =
        export(&ctx, &main_id(), &out, &ExportSettings::new(Format::H264 { crf: 20, preset: "ultrafast".into() }), |_, _| true).unwrap();
    assert!(report.loudness.is_none());
    assert!(probe(&out).unwrap().audio.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn x1_plugin_defined_formats_video_still_and_audio_only() {
    if !have_ffmpeg() {
        return;
    }
    let dir = temp_dir("x1");
    let mut p = moving_project(0.5);
    let wav = dir.join("clicks.wav");
    write_wav(&wav, &click_track(1)).unwrap();
    add_audio_layer(&mut p, &wav, Time::ZERO);
    let clips = FfmpegClips::new(&dir);
    let ctx = ExportContext { clips: Some(&clips), ..ExportContext::new(&p, &NoFootage) };
    let exporter = |video: &[&str], audio: &[&str], ext: &str, audio_only: bool, single: bool| znd_render::plugins::hooks::Exporter {
        key: "test/x".into(),
        plugin: "test".into(),
        label: "テスト".into(),
        extension: ext.into(),
        video_args: video.iter().map(|s| s.to_string()).collect(),
        audio_args: audio.iter().map(|s| s.to_string()).collect(),
        alpha: false,
        even: false,
        audio_only,
        single_frame: single,
        params: vec![serde_json::from_value(serde_json::json!({"id": "q", "label": "画質", "type": "number", "default": 30})).unwrap()],
        description: String::new(),
    };
    // 映像: {q} がパラメータの値に置き換わる
    let ex = exporter(&["-c:v", "libx264", "-crf", "{q}", "-pix_fmt", "yuv420p"], &["-c:a", "aac"], "mp4", false, false);
    let values = std::collections::HashMap::from([("q".to_string(), "20".to_string())]);
    let fmt = Format::from_exporter(&ex, &values);
    assert!(matches!(&fmt, Format::Custom { video_args, .. } if video_args[3] == "20"));
    let mp4 = dir.join("a.mp4");
    export(&ctx, &main_id(), &mp4, &ExportSettings::new(fmt), |_, _| true).unwrap();
    let info = probe_with(&mp4, true).unwrap();
    assert_eq!((info.codec.as_str(), info.frame_count), ("h264", Some(15)));
    assert!(probe(&mp4).unwrap().audio.is_some());
    // 静止画: 範囲の先頭 1 フレームだけ
    let png = dir.join("still.png");
    let settings = ExportSettings { range: Some((7, 8)), ..ExportSettings::new(Format::from_exporter(&exporter(&[], &[], "png", false, true), &Default::default())) };
    assert_eq!(export(&ctx, &main_id(), &png, &settings, |_, _| true).unwrap().frames, 1);
    let (rp, _) = project_with_footage(&png);
    let footage = FfmpegFootage::new(&dir);
    let (got, want) = (to_rgb8(&decoded(&rp, &footage, 0, true), None), to_rgb8(&reference(&p, 7), None));
    // パイプに流すときのディザの分だけずれる
    assert!(got.iter().zip(&want).all(|(a, b)| a.abs_diff(*b) <= 1));
    // 音声だけ: 映像を作らず WAV に
    let out = dir.join("only.wav");
    let ex = exporter(&[], &["-c:a", "pcm_s16le"], "wav", true, false);
    export(&ctx, &main_id(), &out, &ExportSettings::new(Format::from_exporter(&ex, &Default::default())), |_, _| true).unwrap();
    let info = probe(&out).unwrap();
    assert_eq!(info.width, 0);
    assert_eq!(info.audio.unwrap().sample_rate, 48000);
    std::fs::remove_dir_all(dir).unwrap();
}
