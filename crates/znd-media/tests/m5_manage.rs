//! M5 受け入れテスト（入出力側）: 再リンク、ファイル収集、光過敏性の走査、版間差分、分解書き出し、トラッキング。

use std::path::PathBuf;
use znd_core::demo::demo_project;
use znd_core::*;
use znd_media::manage::{collect_files, missing_fonts, missing_footage, relink};
use znd_media::review::{breakdown, changed_frames, export_breakdown, scan_flashes, track, version_diff};
use znd_media::*;
use znd_render::safety::FlashKind;
use znd_render::{FrameBuf, NoFootage, RenderOptions, Renderer};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("znd-m5-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn main_id() -> CompId {
    CompId::new("main")
}

fn png(path: &std::path::Path, rgb: [f32; 3]) {
    let mut b = FrameBuf::new(8, 8);
    b.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 1.0]));
    save_png(&b, path, None).unwrap();
}

/// 静止画フッテージ 2 枚（うち 1 枚はコンポで使用）を持つプロジェクト。
fn footage_project(dir: &std::path::Path) -> Project {
    std::fs::create_dir_all(dir.join("素材/写真")).unwrap();
    png(&dir.join("素材/a.png"), [1.0, 0.0, 0.0]);
    png(&dir.join("素材/写真/b.png"), [0.0, 0.0, 1.0]);
    let mut p = Project::new("収集テスト");
    let a = import_footage(&mut p, &dir.join("素材/a.png")).unwrap();
    let _b = import_footage(&mut p, &dir.join("素材/写真/b.png")).unwrap();
    p.footage[0].rights.author = "作者A".into();
    p.footage[0].rights.copyright = Clearance::Cleared;
    let l = Layer::new("l", "l", LayerKind::Footage { footage: a }, Transform::centered(8.0, 8.0, 8.0, 8.0), Time::from_seconds(1.0));
    p.compositions
        .push(Composition { layers: vec![l], ..Composition::new("main", "本編", 8, 8, FrameRate::FPS_30, Time::from_seconds(1.0)) });
    p
}

#[test]
fn l1_missing_footage_is_detected_and_relinked_by_file_name() {
    let dir = temp_dir("relink");
    let mut p = footage_project(&dir);
    assert!(missing_footage(&p, &dir).is_empty());
    // フォルダごと移動された
    std::fs::rename(dir.join("素材"), dir.join("移動先")).unwrap();
    assert_eq!(missing_footage(&p, &dir).len(), 2);
    let err = Renderer::new(&p, &FfmpegFootage::new(&dir)).render_frame(&main_id(), Time::ZERO, RenderOptions::EXPORT).unwrap_err();
    assert!(err.to_string().contains("a.png"), "{err}");
    // 移動先を指定すると、ファイル名で探して全部つなぎ直す（サブフォルダの中も）
    assert_eq!(relink(&mut p, &dir, &dir.join("移動先")), 2);
    assert!(missing_footage(&p, &dir).is_empty());
    assert!(p.footage[1].path.ends_with("移動先/写真/b.png"));
    let out = Renderer::new(&p, &FfmpegFootage::new(&dir)).render_frame(&main_id(), Time::ZERO, RenderOptions::EXPORT).unwrap();
    assert_eq!(out.pixel(4, 4), [1.0, 0.0, 0.0, 1.0]);
    // 見つからないものはそのまま残る
    std::fs::remove_file(dir.join("移動先/a.png")).unwrap();
    assert_eq!(relink(&mut p, &dir, &dir), 0);
    assert_eq!(missing_footage(&p, &dir), vec![FootageId::new("footage-1")]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn l2_missing_fonts_are_listed() {
    let mut p = demo_project();
    assert!(missing_fonts(&p).is_empty());
    let mut doc = TextDocument::new("x", 20.0);
    doc.font_family = "インストールされていないフォント".into();
    let l = Layer::new(
        "t",
        "t",
        LayerKind::Text { document: doc, animators: vec![], path: None, per_char_3d: false, text_expression: None },
        Transform::centered(0.0, 0.0, 10.0, 10.0),
        Time::from_seconds(1.0),
    );
    p.compositions[1].layers.push(l);
    assert_eq!(missing_fonts(&p), vec!["インストールされていないフォント".to_string()]);
}

#[test]
fn l3_collect_files_gathers_used_footage_with_rights_and_credits() {
    let dir = temp_dir("collect");
    let p = footage_project(&dir);
    // コンポ指定: 使っている素材だけ
    let dest = dir.join("提出用");
    let report = collect_files(&p, &dir, Some(&main_id()), &dest).unwrap();
    assert_eq!((report.copied, report.missing.len()), (1, 0));
    assert!(dest.join("footage/a.png").exists() && !dest.join("footage/b.png").exists());
    // 収集先のプロジェクトは相対パスになっていて、そのまま開いてレンダリングできる
    let collected = io::load(&report.project_path).unwrap();
    assert_eq!(collected.footage[0].path, "footage/a.png");
    assert_eq!(collected.validate(), Vec::<String>::new());
    let out = Renderer::new(&collected, &FfmpegFootage::new(&dest)).render_frame(&main_id(), Time::ZERO, RenderOptions::EXPORT).unwrap();
    assert_eq!(out.pixel(4, 4), [1.0, 0.0, 0.0, 1.0]);
    let rights = std::fs::read_to_string(dest.join("権利情報.txt")).unwrap();
    assert!(rights.contains("a.png") && rights.contains("作者A") && rights.contains("要確認"), "{rights}");
    let credits = std::fs::read_to_string(dest.join("クレジット.txt")).unwrap();
    assert!(credits.contains("【本編】") && credits.contains("作者A"), "{credits}");

    // プロジェクト全体: 未使用の素材も含む。元ファイルが無いものは一覧で知らせる
    std::fs::remove_file(dir.join("素材/写真/b.png")).unwrap();
    let all = collect_files(&p, &dir, None, &dir.join("全部")).unwrap();
    assert_eq!(all.copied, 1);
    assert_eq!(all.missing, vec!["b.png".to_string()]);
    std::fs::remove_dir_all(dir).unwrap();
}

/// 全面平面の不透明度を、停止キーで `hz` 回／秒 点滅させたコンポ。
fn strobe(hz: f64, seconds: f64, color: [f32; 3]) -> Project {
    let dur = Time::from_seconds(seconds);
    let mut l = Layer::new(
        "flash",
        "flash",
        LayerKind::Solid { width: 320, height: 180, color },
        Transform::centered(320.0, 180.0, 320.0, 180.0),
        dur,
    );
    let half = 0.5 / hz;
    l.transform.opacity = Property::constant(0.0).with_keys(
        (0..(seconds / half) as usize)
            .map(|i| Keyframe::hold(Time::from_seconds(i as f64 * half + 1e-4), if i % 2 == 0 { 0.0 } else { 100.0 }))
            .collect(),
    );
    let mut p = Project::new("strobe");
    p.compositions.push(Composition { layers: vec![l], ..Composition::new("main", "main", 320, 180, FrameRate::FPS_30, dur) });
    p
}

#[test]
fn h1_flash_scan_flags_dangerous_ranges_only() {
    let scan = |p: &Project| scan_flashes(&ExportContext::new(p, &NoFootage), &main_id(), |_, _| true).unwrap();
    let w = scan(&strobe(6.0, 2.0, [1.0, 1.0, 1.0]));
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].kind, FlashKind::Luminance);
    assert!(w[0].flashes_per_second >= 5.0);
    assert!(scan(&strobe(2.0, 2.0, [1.0, 1.0, 1.0])).is_empty());
    // デモコンポ（毎秒 1 回のフラッシュ演出）は安全
    assert!(scan(&demo_project()).is_empty());
    // 暗い赤の点滅は赤色フラッシュ
    let red = scan(&strobe(6.0, 2.0, [0.7, 0.0, 0.0]));
    assert!(red.iter().any(|w| w.kind == FlashKind::Red), "{red:?}");
    // 進捗コールバックで中断できる
    let p = strobe(6.0, 2.0, [1.0, 1.0, 1.0]);
    assert!(matches!(scan_flashes(&ExportContext::new(&p, &NoFootage), &main_id(), |_, _| false), Err(ExportError::Cancelled)));
}

#[test]
fn v1_version_diff_localizes_the_change() {
    let a = demo_project();
    let mut b = demo_project();
    // 「衛星 A」の色だけを変える
    if let LayerKind::Solid { color, .. } = &mut b.compositions[0].layer_mut(&LayerId::new("sat-a")).unwrap().kind {
        *color = [0.1, 1.0, 0.1];
    }
    let t = Time::from_seconds(1.0);
    let same = version_diff(&a, &a, &NoFootage, &main_id(), t, 2).unwrap();
    assert_eq!((same.changed_pixels, same.bbox), (0, None));
    let d = version_diff(&a, &b, &NoFootage, &main_id(), t, 2).unwrap();
    // 衛星 A は 1 秒時点で軌道ヌル（回転 72°）の先 420px にある 160px 角の平面。1/2 解像度での位置を求める
    let ang = 72f64.to_radians();
    let (cx, cy) = ((960.0 + 420.0 * ang.cos()) / 2.0, (540.0 + 420.0 * ang.sin()) / 2.0);
    let [x0, y0, x1, y1] = d.bbox.unwrap();
    let half_diag = 160.0 * 2f64.sqrt() / 2.0 / 2.0;
    assert!((x0 as f64) >= cx - half_diag - 2.0 && (x1 as f64) <= cx + half_diag + 2.0, "x {x0}..{x1}, 中心 {cx}");
    assert!((y0 as f64) >= cy - half_diag - 2.0 && (y1 as f64) <= cy + half_diag + 2.0, "y {y0}..{y1}, 中心 {cy}");
    assert!(d.changed_pixels > 3000 && d.changed_pixels < 9000, "{}", d.changed_pixels);
    // 変わったフレームの列挙: レイヤーのイン点を後ろへずらした版では、その区間だけが違う
    let mut c = demo_project();
    c.compositions[0].layer_mut(&LayerId::new("sat-a")).unwrap().in_point = Time::from_seconds(2.0);
    let frames = changed_frames(&a, &c, &NoFootage, &main_id(), 15).unwrap();
    assert_eq!(frames, vec![0, 15, 30, 45]);
    // 解像度の違う版どうしは比較できない
    let mut e = demo_project();
    e.compositions[0].width = 1280;
    assert!(version_diff(&a, &e, &NoFootage, &main_id(), t, 2).is_err());
}

#[test]
fn m1_breakdown_adds_layers_from_the_bottom() {
    let p = demo_project();
    let ctx = ExportContext::new(&p, &NoFootage);
    let t = Time::from_seconds(1.5);
    let stages = breakdown(&ctx, &main_id(), t, 4).unwrap();
    // 描画に関わるレイヤー（マットは非表示、ヌルは除外）
    let names: Vec<&str> = stages.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names, vec!["背景", "オーバーレイ板", "ムーバー", "衛星 B", "衛星 A", "カラーバー", "フラッシュ"]);
    // 最初の段階は背景だけ、最後の段階は完成形と一致
    let bg = &stages[0].1;
    assert!(bg.data.chunks_exact(4).all(|px| px == bg.pixel(0, 0)));
    let full = Renderer::new(&p, &NoFootage).render_frame(&main_id(), t, RenderOptions { downsample: 4, ..RenderOptions::EXPORT }).unwrap();
    assert_eq!(stages.last().unwrap().1, full);
    assert_ne!(stages[1].1, stages[0].1);
    assert_ne!(stages[5].1, stages[4].1);

    let dir = temp_dir("breakdown");
    let small = {
        let mut q = p.clone();
        q.compositions[0].width = 480;
        q.compositions[0].height = 270;
        q
    };
    let files = export_breakdown(&ExportContext::new(&small, &NoFootage), &main_id(), t, &dir, "making").unwrap();
    assert_eq!(files.len(), 7);
    assert!(files[0].ends_with("making_01_背景.png") && files[0].exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn t1_tracking_a_moving_layer_yields_its_path() {
    // 模様のある小さな図形が、曲線を描いて動く
    let dur = Time::from_seconds(1.0);
    let mut target = Layer::new(
        "target",
        "target",
        LayerKind::Shape {
            items: vec![
                ShapeItem::star(5.0, 14.0, 6.0),
                ShapeItem::stroke([0.0, 0.0, 0.0, 1.0], 2.0),
                ShapeItem::fill([1.0, 1.0, 1.0, 1.0]),
            ],
        },
        Transform::centered(0.0, 0.0, 200.0, 120.0),
        dur,
    );
    target.transform.position = Property::spatial([0.0, 0.0]).with_keys(vec![
        Keyframe::linear(Time::ZERO, [40.0, 80.0]).with_auto_path(),
        Keyframe::linear(Time::from_seconds(0.5), [100.0, 40.0]).with_auto_path(),
        Keyframe::linear(dur, [160.0, 80.0]).with_auto_path(),
    ]);
    let bg = Layer::new(
        "bg",
        "bg",
        LayerKind::Solid { width: 200, height: 120, color: [0.2, 0.3, 0.5] },
        Transform::centered(200.0, 120.0, 200.0, 120.0),
        dur,
    );
    let mut p = Project::new("track");
    p.compositions
        .push(Composition { layers: vec![target.clone(), bg], ..Composition::new("main", "main", 200, 120, FrameRate::FPS_30, dur) });
    let result = track(&ExportContext::new(&p, &NoFootage), &main_id(), 0, 29, [40.0, 80.0], 12, 8).unwrap();
    assert_eq!(result.len(), 30);
    for (t, pos) in &result {
        let truth = target.transform.position.eval(*t);
        assert!((pos[0] - truth[0]).abs() < 0.5 && (pos[1] - truth[1]).abs() < 0.5, "{:.3}s: {pos:?} vs {truth:?}", t.seconds());
    }
}
