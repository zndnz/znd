//! 読み込み形式の受け入れテスト: 連番・TIFF/TGA・GIF/APNG・SVG・PDF/AI・PSD。

use std::path::{Path, PathBuf};
use znd_core::shape::ShapeItem;
use znd_core::*;
use znd_media::*;
use znd_render::color::linear_to_srgb;
use znd_render::{FootageSource, FrameBuf, NoFootage, RenderOptions, Renderer};

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("znd-import-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn solid_png(path: &Path, rgb: [u8; 3]) {
    let px: Vec<u8> = (0..16 * 12).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect();
    image::save_buffer(path, &px, 16, 12, image::ExtendedColorType::Rgba8).unwrap();
}

/// 画素の表示値（sRGB 8bit 相当、ストレート）。
fn display(buf: &FrameBuf, x: usize, y: usize) -> [i32; 4] {
    let p = buf.pixel(x, y);
    let a = p[3].max(1e-6);
    let c = |v: f32| (linear_to_srgb((v / a).clamp(0.0, 1.0)) * 255.0).round() as i32;
    [c(p[0]), c(p[1]), c(p[2]), (p[3] * 255.0).round() as i32]
}

fn near(got: [i32; 4], want: [i32; 4], tol: i32) {
    assert!(got.iter().zip(&want).all(|(g, w)| (g - w).abs() <= tol), "got {got:?}, want {want:?}");
}

/// ベクター素材を、アートボードと同じ大きさのコンポに置いて描く（背景は透明）。
fn render_vector(v: VectorImport) -> FrameBuf {
    let (w, h) = (v.width.round() as u32, v.height.round() as u32);
    let d = Time::from_seconds(1.0);
    let mut comp = Composition::new("c", "c", w, h, FrameRate::FPS_30, d);
    comp.layers.push(v.into_layer("v", w as f64, h as f64, d));
    let mut p = Project::new("t");
    p.compositions.push(comp);
    let opts = RenderOptions { downsample: 1, background: false, guides: false };
    Renderer::new(&p, &NoFootage).render_frame(&CompId::new("c"), Time::ZERO, opts).unwrap()
}

#[test]
fn i1_image_sequence_plays_as_footage_and_fills_gaps() {
    let dir = temp_dir("seq");
    // 3 番が抜けた連番（10 始まり）
    for (n, c) in [(10, [255, 0, 0]), (11, [0, 255, 0]), (13, [0, 0, 255])] {
        solid_png(&dir.join(format!("shot_{n:04}.png")), c);
    }
    let mut p = Project::new("t");
    let (id, warnings) = import_sequence(&mut p, &dir.join("shot_0011.png"), FrameRate { num: 10, den: 1 }).unwrap();
    let item = p.footage_item(&id).unwrap().clone();
    assert!(item.path.ends_with("shot_%04d.png"));
    assert_eq!((item.width, item.height), (16, 12));
    assert_eq!(item.duration, Some(Time::from_seconds(0.4)));
    assert!(warnings.iter().any(|w| w.contains("抜け")), "{warnings:?}");

    let src = FfmpegFootage::new(&dir);
    let at = |s: f64| display(&src.frame(&item, s).unwrap().unwrap(), 3, 3);
    near(at(0.0), [255, 0, 0, 255], 0);
    near(at(0.15), [0, 255, 0, 255], 0);
    // 抜けたコマは直前のコマ
    near(at(0.25), [0, 255, 0, 255], 0);
    near(at(0.35), [0, 0, 255, 255], 0);
    assert!(src.frame(&item, 0.45).unwrap().is_none());
    // 1 枚しかないファイルは連番にならない
    solid_png(&dir.join("single_7.png"), [1, 2, 3]);
    assert!(import_sequence(&mut p, &dir.join("single_7.png"), FrameRate::FPS_30).is_err());
}

#[test]
fn i2_tiff_and_tga_stills() {
    let dir = temp_dir("stills");
    let px: Vec<u8> = (0..8 * 8).flat_map(|_| [200u8, 100, 50, 255]).collect();
    for name in ["a.tif", "a.tga", "a.bmp"] {
        let path = dir.join(name);
        image::save_buffer(&path, &px, 8, 8, image::ExtendedColorType::Rgba8).unwrap();
        let mut p = Project::new("t");
        let id = import_footage(&mut p, &path).unwrap();
        let item = p.footage_item(&id).unwrap().clone();
        assert_eq!((item.width, item.height, item.frame_rate), (8, 8, None), "{name}");
        let f = FfmpegFootage::new(&dir).frame(&item, 0.0).unwrap().unwrap();
        near(display(&f, 4, 4), [200, 100, 50, 255], 0);
    }
}

#[test]
fn i3_gif_and_apng_are_read_as_animation() {
    if znd_media::tools::ffmpeg().is_err() {
        eprintln!("ffmpeg が無いためスキップ");
        return;
    }
    let dir = temp_dir("anim");
    for (n, c) in [(0, [255, 0, 0]), (1, [0, 255, 0]), (2, [0, 0, 255]), (3, [255, 255, 255])] {
        solid_png(&dir.join(format!("f_{n}.png")), c);
    }
    let ffmpeg = znd_media::tools::ffmpeg().unwrap();
    let run = |args: &[&str], out: &Path| {
        let st = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-y", "-framerate", "10", "-i"])
            .arg(dir.join("f_%d.png"))
            .args(args)
            .arg(out)
            .status()
            .unwrap();
        assert!(st.success());
    };
    run(&[], &dir.join("a.gif"));
    run(&["-f", "apng", "-plays", "0"], &dir.join("a.apng"));
    // 拡張子が .png でも、中身がアニメーション PNG なら動画として扱う
    std::fs::copy(dir.join("a.apng"), dir.join("anim.png")).unwrap();
    assert!(znd_media::probe::is_apng(&dir.join("anim.png")));
    assert!(!znd_media::probe::is_apng(&dir.join("f_0.png")));

    for name in ["a.gif", "a.apng", "anim.png"] {
        let mut p = Project::new("t");
        let id = import_footage(&mut p, &dir.join(name)).unwrap();
        let item = p.footage_item(&id).unwrap().clone();
        assert!(item.frame_rate.is_some(), "{name} は動画として読み込まれる");
        let d = item.duration.unwrap().seconds();
        assert!((d - 0.4).abs() < 0.11, "{name}: 尺 {d}");
        let src = FfmpegFootage::new(&dir);
        let first = display(&src.frame(&item, 0.01).unwrap().unwrap(), 4, 4);
        let third = display(&src.frame(&item, 0.25).unwrap().unwrap(), 4, 4);
        // GIF はパレット化されるので許容を広めに取る
        near(first, [255, 0, 0, 255], 12);
        near(third, [0, 0, 255, 255], 12);
    }
}

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80" viewBox="0 0 100 80">
  <rect x="10" y="10" width="30" height="20" fill="#ff0000"/>
  <g transform="translate(50,40) scale(2)" opacity="0.5">
    <circle cx="10" cy="5" r="5" fill="none" stroke="#00ff00" stroke-width="1"/>
  </g>
  <path d="M60 5 h30 v30 h-30 z M70 15 h10 v10 h-10 z" fill-rule="evenodd" fill="#0000ff"/>
  <linearGradient id="g" gradientUnits="userSpaceOnUse" x1="10" y1="0" x2="40" y2="0">
    <stop offset="0" stop-color="#000000"/><stop offset="1" stop-color="#ffffff"/>
  </linearGradient>
  <rect x="10" y="60" width="30" height="10" fill="url(#g)"/>
</svg>"##;

#[test]
fn i4_svg_becomes_vector_shape_layer() {
    let dir = temp_dir("svg");
    let path = dir.join("art.svg");
    std::fs::write(&path, SVG).unwrap();
    assert!(is_vector(&path));
    let v = import_vector(&path).unwrap();
    assert_eq!((v.width, v.height, v.path_count), (100.0, 80.0, 4));
    // ラスタライズされず、パスのまま入っている
    let paths = v
        .items
        .iter()
        .filter(|i| matches!(i, ShapeItem::Group { items, .. } if items.iter().any(|x| matches!(x, ShapeItem::Path { .. }))))
        .count();
    assert_eq!(paths, 4);

    let buf = render_vector(v.clone());
    near(display(&buf, 25, 20), [255, 0, 0, 255], 1);
    assert!(buf.pixel(5, 5)[3] < 1e-4);
    // 偶奇規則の穴
    near(display(&buf, 65, 10), [0, 0, 255, 255], 1);
    assert!(buf.pixel(75, 20)[3] < 1e-4);
    // 変換行列つきの円の線（中心 (70,50)・半径 10・線幅 2、グループの不透明度 50%）
    let ring = buf.pixel(80, 50);
    assert!((ring[3] - 0.5).abs() < 0.02 && ring[1] > ring[0], "{ring:?}");
    assert!(buf.pixel(70, 50)[3] < 1e-4);
    // グラデーション: 左が暗く、右が明るい
    let (l, r) = (display(&buf, 12, 65)[0], display(&buf, 38, 65)[0]);
    assert!(l < 40 && r > 215, "{l} {r}");

    // 拡大しても縁が滲まない（4 倍にしたコンポで、縁の遷移が 1 画素に収まる）
    let d = Time::from_seconds(1.0);
    let mut layer = v.into_layer("v", 400.0, 320.0, d);
    layer.transform.scale = Property::constant([400.0, 400.0]);
    let mut comp = Composition::new("c", "c", 400, 320, FrameRate::FPS_30, d);
    comp.layers.push(layer);
    let mut p = Project::new("t");
    p.compositions.push(comp);
    let big = Renderer::new(&p, &NoFootage)
        .render_frame(&CompId::new("c"), Time::ZERO, RenderOptions { downsample: 1, background: false, guides: false })
        .unwrap();
    assert!(big.pixel(38, 60)[3] < 1e-4);
    assert!(big.pixel(41, 60)[3] > 0.9999);
}

/// 赤い矩形（cm つき）、青い線、Form XObject の中の緑の矩形、CMYK の塗りを持つ 100x80pt の PDF。
fn sample_pdf() -> Vec<u8> {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let form = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 10.into(), 10.into()],
        "Matrix" => vec![1.into(), 0.into(), 0.into(), 1.into(), 80.into(), 60.into()] },
        b"0 1 0 rg 0 0 10 10 re f".to_vec(),
    );
    let form_id = doc.add_object(form);
    let content = b"q 1 0 0 1 10 10 cm 1 0 0 rg 0 0 30 20 re f Q\n0 0 1 RG 4 w 50 50 m 90 50 l S\n/Fm0 Do\n0 1 1 0 k 10 60 m 20 60 l 20 70 l 10 70 l h f\n";
    let mut stream = Stream::new(dictionary! {}, content.to_vec());
    let _ = stream.compress();
    let content_id = doc.add_object(stream);
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Fm0" => form_id } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 100.into(), 80.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn i5_pdf_and_pdf_compatible_ai_become_vector_shape_layers() {
    let dir = temp_dir("pdf");
    let bytes = sample_pdf();
    for name in ["art.pdf", "art.ai"] {
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let v = import_vector(&path).unwrap();
        assert_eq!((v.width, v.height, v.path_count), (100.0, 80.0, 4), "{name}");
        let buf = render_vector(v);
        // PDF は左下原点: 赤い矩形は x 10..40, y(上から) 50..70
        near(display(&buf, 25, 60), [255, 0, 0, 255], 1);
        assert!(buf.pixel(25, 45)[3] < 1e-4);
        // 青い線: y = 30 を中心に幅 4
        near(display(&buf, 70, 30), [0, 0, 255, 255], 1);
        assert!(buf.pixel(70, 34)[3] < 1e-4);
        // Form XObject（Matrix で (80,60) へ）: x 80..90, y(上から) 10..20
        near(display(&buf, 85, 15), [0, 255, 0, 255], 1);
        // CMYK (0,1,1,0) = 赤: x 10..20, y(上から) 10..20
        near(display(&buf, 15, 15), [255, 0, 0, 255], 1);
    }
    // PDF 互換でない AI は、理由と対処を示して断る
    let old = dir.join("old.ai");
    std::fs::write(&old, b"%!PS-Adobe-3.0\n%%Creator: Adobe Illustrator\n").unwrap();
    let e = import_vector(&old).unwrap_err().to_string();
    assert!(e.contains("PDF 互換"), "{e}");
}

#[test]
fn i6_psd_keeps_layer_structure() {
    let dir = temp_dir("psd");
    let path = dir.join("layers.psd");
    std::fs::copy(fixture("layers.psd"), &path).unwrap();
    let mut p = Project::new("t");
    let (comp_id, warnings) = import_psd(&mut p, &path, FrameRate::FPS_30, Time::from_seconds(2.0)).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let comp = p.comp(&comp_id).unwrap();
    assert_eq!((comp.width, comp.height, comp.layers.len()), (64, 48, 4));
    // 先頭が最前面
    let names: Vec<&str> = comp.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["grp / hidden", "grp / circle", "red", "背景"]);
    let red = &comp.layers[2];
    assert_eq!(red.blend_mode, BlendMode::Multiply);
    assert!((red.transform.opacity.eval(Time::ZERO) - 128.0 / 255.0 * 100.0).abs() < 0.01);
    assert_eq!(red.transform.position.eval(Time::ZERO), [18.0, 9.0]);
    assert!(!comp.layers[0].visible && comp.layers[1].visible);
    assert_eq!(p.footage.len(), 4);
    assert!(dir.join("layers_layers").is_dir());

    let src = FfmpegFootage::new(&dir);
    let buf = Renderer::new(&p, &src).render_frame(&comp_id, Time::ZERO, RenderOptions::EXPORT).unwrap();
    near(display(&buf, 60, 40), [10, 20, 200, 255], 1);
    // 円（ZIP 圧縮のレイヤー）: 中心は緑、角は背景
    near(display(&buf, 38, 28), [0, 255, 0, 255], 1);
    near(display(&buf, 30, 20), [10, 20, 200, 255], 1);
    // 非表示レイヤーは描かれない
    near(display(&buf, 3, 3), [10, 20, 200, 255], 1);
    // 乗算・約 50%: 青が暗くなる（赤×青 = 黒との中間）
    let m = display(&buf, 15, 8);
    assert!(m[2] < 190 && m[2] > 60 && m[0] <= 12, "{m:?}");
}

#[test]
fn i6_psd_errors_are_explained() {
    assert!(parse_psd(b"not a psd").unwrap_err().contains("PSD"));
    // CMYK
    let mut h = b"8BPS\x00\x01\x00\x00\x00\x00\x00\x00\x00\x04\x00\x00\x00\x08\x00\x00\x00\x08\x00\x08\x00\x04".to_vec();
    h.extend([0u8; 12]);
    assert!(parse_psd(&h).unwrap_err().contains("CMYK"));
}
