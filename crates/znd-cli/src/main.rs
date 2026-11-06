//! znd コマンドラインツール。検査・レンダリング・書き出し・提出前チェック・素材管理・一括処理。

use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;
use znd_audio::analysis::{band_keyframes, detect_onsets, Band};
use znd_audio::Mixer;
use znd_core::demo::demo_project;
use znd_core::*;
use znd_media::manage::{collect_files, missing_fonts, missing_footage, relink};
use znd_media::review::{changed_frames, export_breakdown, scan_flashes, version_diff};
use znd_media::{
    abc_camera_layer, export, import_file, make_proxy, notify, run_queue, save_png, ExportContext, ExportSettings, FfmpegClips,
    FfmpegFootage, Format, ImportOptions, QueueEvent, QueueJob,
};
use znd_render::{plugins, NoFootage, RenderOptions, Renderer};
use znd_script::{run_script, ExprEngine};

const USAGE: &str = "\
znd — レイヤーベース合成・モーショングラフィックスエンジン

基本:
  znd demo <出力.znd.json>                 デモプロジェクトを書き出す
  znd info <プロジェクト>                   構成を表示する
  znd validate <プロジェクト>               参照切れ・循環を検査する
  znd render <プロジェクト> -o <出力.png> [--comp ID] [--frame N] [--scale 1|2|4] [--alpha] [--gpu]
  znd export <プロジェクト> -o <出力> [--comp ID] [--format 形式] [--crf N] [--preset 名前]
             [--two-pass kbps] [--range 開始:終了] [--pad 秒] [--scale N] [--no-audio] [--alpha] [--report] [--gpu]
             --gpu: エフェクトを GPU で処理する（既定は CPU。結果は同じで、差は 8bit の 1 段未満）
             形式: h264（既定）/ h265 / prores / prores4444 / png / png16 / exr / gif / screening（上映提出用）
                   plugin:<パッケージID/ID>（プラグインの書き出し形式。--param 名前=値 で設定）
  znd queue <プロジェクト> <キュー.json>     レンダーキュー（QueueJob の配列）を順に書き出す

提出前チェック:
  znd check <プロジェクト> [--comp ID]      権利・欠落素材・エクスプレッション・明滅・ラウドネスをまとめて検査
  znd credits <プロジェクト> [--comp ID]    クレジット文面を出力する
  znd collect <プロジェクト> <収集先> [--comp ID]   素材と権利情報を 1 フォルダに集める
  znd relink <プロジェクト> <探すフォルダ>   欠落フッテージをファイル名で探してつなぎ直す

音・歌詞:
  znd beats <プロジェクト> --layer ID [--comp ID] [--sensitivity 0..1]   オンセットを検出してマーカーを打つ
  znd band-keys <プロジェクト> --layer ID --target ID [--band low|mid|high] [--comp ID]
                                           帯域の音量を、対象レイヤーの不透明度キーフレームにする
  znd lyrics <プロジェクト> <歌詞.lrc|srt|ass> [--comp ID] [--size px]   歌詞を行ごとのテキストレイヤーにする

版管理・共同制作・振り返り:
  znd snapshot <プロジェクト> <名前>        版を保存する（引数なしで一覧）
  znd diff <版A> <版B> [--comp ID] [--frame N] [-o 差分.png]   版の違いを調べる
  znd export-comp <プロジェクト> <コンポID> -o <パッケージ.json>
  znd import-comp <プロジェクト> <パッケージ.json>
  znd breakdown <プロジェクト> -o <フォルダ> [--comp ID] [--frame N]   レイヤーを 1 枚ずつ重ねた連番を書き出す
  znd stats <プロジェクト>                  制作ログとコンポ別の統計

読み込み:
  znd import <プロジェクト> <ファイル> [--comp ID] [--sequence] [--fps N]
             動画・静止画（PNG/JPEG/EXR/TIFF/TGA）・GIF/APNG・音声・連番画像・PSD（レイヤー保持）・
             SVG/PDF/AI（ベクター保持）・3D モデル（glTF/OBJ/FBX/Alembic）をレイヤーとして読み込む

プラグイン:
  znd plugins [ファイル]                    読み込まれるプラグイン（WebAssembly）とパラメータの一覧。
                                           ファイルを指定すると、それだけを試しに読み込む（開発時の確認用）
                                           エフェクトの WebAssembly はサンドボックスで動く。サンドボックスの外で動く
                                           ネイティブ（znd ABI・OpenFX）は ZND_ALLOW_NATIVE_PLUGINS=1 のときだけ読み込む
  znd plugin-scan <ファイル|フォルダ>        読み込まずに安全性スキャンだけ行い、危険度と根拠を表示する
                                           （外部サーバーとの通信・情報の持ち出し・プログラムの実行など）

その他:
  znd script <プロジェクト> <スクリプト.js>  自動化スクリプトを実行して保存する
  znd camera-import <プロジェクト> <カメラ.chan|.abc> [--comp ID] [--scale px/単位]
  znd proxy <プロジェクト> [--scale N]      動画フッテージのプロキシを生成する
  znd bench [--layers N] [--effects N] [--full] [--scale 1|2|4] [--frames N] [--gpu]
  znd licenses
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("エラー: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--name 値` 形式のオプションを取り出す。
fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn opt_num<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    match opt(args, name) {
        Some(s) => s.parse().map_err(|_| format!("{name} の値「{s}」が不正です")),
        None => Ok(default),
    }
}

/// オプションではない n 番目の引数（`--x 値` の値は飛ばす）。
fn positional(args: &[String], n: usize) -> Option<&String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with('-') {
            // 値を取らないフラグ
            if !matches!(args[i].as_str(), "--alpha" | "--no-audio" | "--report" | "--sequence" | "--gpu" | "--full") {
                i += 1;
            }
        } else {
            out.push(&args[i]);
        }
        i += 1;
    }
    out.get(n).copied()
}

/// `--gpu`（または環境変数 `ZND_GPU=1`）が指定されていれば、エフェクトを GPU で処理する。
/// GPU が使えなければ理由を表示して CPU で続ける。
fn gpu(args: &[String]) -> Option<&'static dyn znd_render::effects::EffectAccel> {
    gpu_backend(args).map(|g| g as &dyn znd_render::effects::EffectAccel)
}

/// `--gpu` のときは合成も GPU で行う（レイヤーのソースを送り、完成したフレームだけを読み戻す）。
fn gpu_composite(args: &[String]) -> Option<&'static dyn znd_render::gpu_composite::CompositeAccel> {
    gpu_backend(args).map(|g| g as &dyn znd_render::gpu_composite::CompositeAccel)
}

fn gpu_backend(args: &[String]) -> Option<&'static znd_gpu::GpuBackend> {
    static GPU: std::sync::OnceLock<Option<znd_gpu::GpuBackend>> = std::sync::OnceLock::new();
    let wanted = flag(args, "--gpu") || std::env::var("ZND_GPU").is_ok_and(|v| v == "1");
    if !wanted {
        return None;
    }
    GPU.get_or_init(|| match znd_gpu::GpuBackend::new() {
        Ok(g) => {
            eprintln!("GPU: {}", g.name());
            Some(g)
        }
        Err(e) => {
            eprintln!("GPU を使えません（CPU で処理します）: {e}");
            None
        }
    })
    .as_ref()
}

fn load(path: Option<&String>) -> Result<(Project, PathBuf), String> {
    let path = PathBuf::from(path.ok_or("プロジェクトファイルを指定してください")?);
    // プラグインのエフェクトを使えるようにしておく
    for (file, e) in plugins::scan_default().errors {
        eprintln!("プラグイン {}: {e}", file.display());
    }
    let project = io::load(&path).map_err(|e| e.to_string())?;
    Ok((project, path))
}

fn save(project: &Project, path: &Path) -> Result<(), String> {
    io::save(project, path).map_err(|e| e.to_string())
}

/// `--comp` で指定されたコンポ、無ければ先頭のコンポ。
fn pick_comp<'a>(project: &'a Project, args: &[String]) -> Result<&'a Composition, String> {
    match opt(args, "--comp") {
        Some(id) => project.comp(&CompId::new(id)).ok_or_else(|| format!("コンポ「{id}」が見つかりません")),
        None => project.compositions.first().ok_or_else(|| "コンポジションがありません".to_string()),
    }
}

fn base_dir(project_path: &Path) -> PathBuf {
    project_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf()
}

fn parse_format(args: &[String]) -> Result<ExportSettings, String> {
    let crf = |d: u8| opt_num(args, "--crf", d);
    let preset = |d: &str| opt(args, "--preset").unwrap_or(d).to_string();
    let mut s = match opt(args, "--format").unwrap_or("h264") {
        "h264" => ExportSettings::new(Format::H264 { crf: crf(14)?, preset: preset("slow") }),
        "h265" => {
            let two_pass =
                opt(args, "--two-pass").map(|v| v.parse::<u32>().map_err(|_| format!("--two-pass の値「{v}」が不正です"))).transpose()?;
            ExportSettings::new(Format::H265 { crf: crf(18)?, preset: preset("medium"), two_pass_kbps: two_pass })
        }
        "prores" => ExportSettings::new(Format::ProRes422),
        "prores4444" => ExportSettings::new(Format::ProRes4444),
        "png" => ExportSettings::new(Format::PngSequence { sixteen_bit: false }),
        "png16" => ExportSettings::new(Format::PngSequence { sixteen_bit: true }),
        "exr" => ExportSettings::new(Format::ExrSequence),
        "gif" => ExportSettings::new(Format::Gif),
        "screening" => ExportSettings::screening(1.0),
        other if other.starts_with("plugin:") => {
            let key = &other["plugin:".len()..];
            let list = znd_render::plugins::hooks::exporters();
            let ex = list.iter().find(|e| e.key == key || e.key.ends_with(&format!("/{key}"))).ok_or_else(|| {
                let names: Vec<&str> = list.iter().map(|e| e.key.as_str()).collect();
                format!("プラグインの書き出し形式「{key}」がありません（あるもの: {}）", if names.is_empty() { "なし".into() } else { names.join(", ") })
            })?;
            // --param 名前=値 を並べて指定する
            let mut values = std::collections::HashMap::new();
            for (i, a) in args.iter().enumerate() {
                if a == "--param" {
                    let kv = args.get(i + 1).ok_or("--param には 名前=値 を続けてください")?;
                    let (k, v) = kv.split_once('=').ok_or("--param は 名前=値 で指定してください")?;
                    values.insert(k.to_string(), v.to_string());
                }
            }
            ExportSettings::new(Format::from_exporter(ex, &values))
        }
        other => return Err(format!("未対応の形式です: {other}")),
    };
    if let Some(r) = opt(args, "--range") {
        let (a, b) = r.split_once(':').ok_or("--range は「開始:終了」のフレーム番号で指定してください")?;
        s.range = Some((a.parse().map_err(|_| "--range の開始が不正です")?, b.parse().map_err(|_| "--range の終了が不正です")?));
    }
    if let Some(p) = opt(args, "--pad") {
        let v: f64 = p.parse().map_err(|_| "--pad の値が不正です")?;
        s.pad_head = v;
        s.pad_tail = v;
    }
    s.downsample = opt_num(args, "--scale", s.downsample)?;
    s.audio = !flag(args, "--no-audio");
    s.transparent = flag(args, "--alpha");
    Ok(s)
}

fn progress_line(done: i64, total: i64) -> bool {
    eprint!("\r{done}/{total} フレーム");
    true
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "demo" => {
            let out = rest.first().ok_or("出力ファイルを指定してください")?;
            save(&demo_project(), Path::new(out))?;
            println!("{out} を書き出しました");
            Ok(())
        }
        "info" => {
            let (project, _) = load(positional(rest, 0))?;
            println!("プロジェクト「{}」（形式 v{}）", project.name, project.format_version);
            let s = &project.settings;
            println!(
                "合成: {} / 色深度 {:?} / 作業色空間 {:?} → 出力 {:?}",
                if s.linear_blending { "リニアライト" } else { "表示ガンマ" },
                s.bit_depth,
                s.working_space,
                s.output_space
            );
            for f in &project.footage {
                let audio = f.audio.map(|a| format!(" 音声 {}Hz {}ch", a.sample_rate, a.channels)).unwrap_or_default();
                println!(
                    "  フッテージ {}: {} {}x{}{}{} [{}]",
                    f.id,
                    f.name,
                    f.width,
                    f.height,
                    audio,
                    if f.vfr { " VFR" } else { "" },
                    f.path
                );
            }
            for c in &project.compositions {
                let lock = c.locked_by.as_ref().map(|u| format!("（{u} がロック中）")).unwrap_or_default();
                println!(
                    "  コンポ {}: 「{}」 {}x{} {:.3}fps {} フレーム, レイヤー {} 枚{}",
                    c.id,
                    c.name,
                    c.width,
                    c.height,
                    c.frame_rate.fps(),
                    c.frame_count(),
                    c.layers.len(),
                    lock
                );
            }
            Ok(())
        }
        "validate" => {
            let (project, _) = load(positional(rest, 0))?;
            let issues = project.validate();
            if issues.is_empty() {
                println!("問題は見つかりませんでした");
                Ok(())
            } else {
                Err(format!("{} 件の問題:\n  {}", issues.len(), issues.join("\n  ")))
            }
        }
        "render" => {
            let (project, path) = load(positional(rest, 0))?;
            let comp = pick_comp(&project, rest)?;
            let out = opt(rest, "-o").ok_or("-o で出力ファイルを指定してください")?;
            let frame: i64 = opt_num(rest, "--frame", 0)?;
            let opts = RenderOptions { downsample: opt_num(rest, "--scale", 1)?, background: !flag(rest, "--alpha"), guides: false };
            let dir = base_dir(&path);
            znd_render::effects::set_base_dir(&dir);
            let footage = FfmpegFootage::new(&dir);
            let clips = FfmpegClips::new(&dir);
            let mixer = Mixer::new(&project, &clips);
            let engine = ExprEngine::new();
            let renderer =
                Renderer::new(&project, &footage).with_resolver(&engine).with_audio(&mixer).with_base_dir(&dir).with_accel(gpu(rest)).with_composite(gpu_composite(rest));
            let start = Instant::now();
            let buf = renderer.render_frame(&comp.id, comp.frame_rate.frame_to_time(frame), opts).map_err(|e| e.to_string())?;
            let elapsed = start.elapsed();
            save_png(&buf, Path::new(out), Some(frame as u32)).map_err(|e| e.to_string())?;
            println!("{out}: {}x{}（{:.1} ms）", buf.width, buf.height, elapsed.as_secs_f64() * 1000.0);
            Ok(())
        }
        "export" => {
            let (project, path) = load(positional(rest, 0))?;
            let comp = pick_comp(&project, rest)?;
            let out = PathBuf::from(opt(rest, "-o").ok_or("-o で出力ファイルを指定してください")?);
            let issues = project.validate();
            if !issues.is_empty() {
                return Err(format!("プロジェクトに問題があります:\n  {}", issues.join("\n  ")));
            }
            let settings = parse_format(rest)?;
            let dir = base_dir(&path);
            znd_render::effects::set_base_dir(&dir);
            let footage = FfmpegFootage::new(&dir);
            let clips = FfmpegClips::new(&dir);
            let engine = ExprEngine::new();
            let ctx = ExportContext {
                project: &project,
                footage: &footage,
                clips: Some(&clips),
                resolver: Some(&engine),
                base_dir: Some(dir),
                accel: gpu(rest),
                composite: gpu_composite(rest),
            };
            let start = Instant::now();
            let report = export(&ctx, &comp.id, &out, &settings, progress_line).map_err(|e| format!("\n{e}"))?;
            let secs = start.elapsed().as_secs_f64();
            eprintln!();
            println!(
                "{} を書き出しました（{} フレーム, {secs:.1} 秒, {:.1} fps）",
                out.display(),
                report.frames,
                report.frames as f64 / secs
            );
            for w in &report.warnings {
                println!("警告: {w}");
            }
            if flag(rest, "--report") || opt(rest, "--format") == Some("screening") {
                let txt = out.with_extension("report.txt");
                std::fs::write(&txt, report.to_text(comp)).map_err(|e| e.to_string())?;
                println!("レポート: {}", txt.display());
            }
            notify("znd", &format!("{} の書き出しが終わりました", out.display()));
            Ok(())
        }
        "queue" => {
            let (project, path) = load(positional(rest, 0))?;
            let file = positional(rest, 1).ok_or("キューファイル（JSON）を指定してください")?;
            let jobs: Vec<QueueJob> =
                serde_json::from_str(&std::fs::read_to_string(file).map_err(|e| e.to_string())?).map_err(|e| format!("{file}: {e}"))?;
            let dir = base_dir(&path);
            let footage = FfmpegFootage::new(&dir);
            let clips = FfmpegClips::new(&dir);
            let engine = ExprEngine::new();
            let ctx = ExportContext {
                project: &project,
                footage: &footage,
                clips: Some(&clips),
                resolver: Some(&engine),
                base_dir: Some(dir),
                accel: gpu(rest),
                composite: gpu_composite(rest),
            };
            let results = run_queue(&ctx, &jobs, |e| {
                match e {
                    QueueEvent::Started { index } => eprintln!("[{}/{}] {}", index + 1, jobs.len(), jobs[index].path.display()),
                    QueueEvent::Progress { done, total, .. } => {
                        progress_line(done, total);
                    }
                    QueueEvent::Finished { result, .. } => match result {
                        Ok(_) => eprintln!("\n  完了"),
                        Err(e) => eprintln!("\n  失敗: {e}"),
                    },
                }
                true
            });
            let failed = results.iter().filter(|r| r.is_err()).count();
            notify("znd", &format!("レンダーキューが終わりました（{} 件中 {} 件失敗）", results.len(), failed));
            if failed > 0 {
                return Err(format!("{failed} 件の書き出しに失敗しました"));
            }
            Ok(())
        }
        "check" => {
            let (project, path) = load(positional(rest, 0))?;
            let comp = pick_comp(&project, rest)?;
            let dir = base_dir(&path);
            let mut problems = 0;
            let mut section = |title: &str, lines: Vec<String>| {
                if lines.is_empty() {
                    println!("✓ {title}: 問題なし");
                } else {
                    println!("✗ {title}:");
                    for l in &lines {
                        println!("    {l}");
                    }
                    problems += lines.len();
                }
            };
            section("構造", project.validate());
            section("権利", rights::export_warnings(&project, &comp.id));
            section(
                "プラグイン",
                plugins::missing(&project)
                    .iter()
                    .map(|(kind, layer)| format!("{layer}: プラグイン「{kind}」が入っていません（このエフェクトは無視されます）"))
                    .collect(),
            );
            let missing: Vec<String> = missing_footage(&project, &dir)
                .iter()
                .filter_map(|id| project.footage_item(id))
                .map(|f| format!("ファイルがありません: {}", f.path))
                .collect();
            section("欠落フッテージ", missing);
            section("欠落フォント", missing_fonts(&project).into_iter().map(|f| format!("フォントがありません: {f}")).collect());
            let engine = ExprEngine::new();
            section("エクスプレッション", engine.check(&project, &comp.id, Time::ZERO).iter().map(|e| e.to_string()).collect());
            let res = [(1920, 1080)];
            section(
                "解像度",
                if res.contains(&(comp.width, comp.height)) {
                    vec![]
                } else {
                    vec![format!("{}x{} です（上映提出は 1920x1080 が基本）", comp.width, comp.height)]
                },
            );

            let footage = FfmpegFootage::new(&dir);
            let clips = FfmpegClips::new(&dir);
            let ctx = ExportContext {
                project: &project,
                footage: &footage,
                clips: Some(&clips),
                resolver: Some(&engine),
                base_dir: Some(dir.clone()),
                accel: gpu(rest),
                composite: gpu_composite(rest),
            };
            let flashes = scan_flashes(&ctx, &comp.id, |d, t| {
                eprint!("\r明滅を解析中 {d}/{t}");
                true
            })
            .map_err(|e| e.to_string())?;
            eprint!("\r                              \r");
            let fr = comp.frame_rate;
            section(
                "光過敏性",
                flashes
                    .iter()
                    .map(|w| {
                        format!(
                            "{:.2}〜{:.2} 秒: {}",
                            fr.frame_to_time(w.start_frame).seconds(),
                            fr.frame_to_time(w.end_frame).seconds(),
                            w.message()
                        )
                    })
                    .collect(),
            );

            let mixer = Mixer::new(&project, &clips);
            let mixed = mixer.mix(&comp.id, 0, mixer.comp_frames(comp))?;
            if mixed.iter().any(|v| *v != 0.0) {
                let l = znd_audio::loudness::analyze(&mixed);
                match l.integrated_lufs {
                    Some(v) => println!("  ラウドネス {v:.1} LUFS / トゥルーピーク {:.1} dBTP", l.true_peak_db),
                    None => println!("  ラウドネス 計測不能 / トゥルーピーク {:.1} dBTP", l.true_peak_db),
                }
                section("音量", l.warnings(-16.0, -1.0));
            } else {
                println!("  音声なし");
            }
            if problems > 0 {
                return Err(format!("{problems} 件の指摘があります"));
            }
            Ok(())
        }
        "credits" => {
            let (project, _) = load(positional(rest, 0))?;
            print!("{}", rights::credits(&project, &pick_comp(&project, rest)?.id));
            Ok(())
        }
        "collect" => {
            let (project, path) = load(positional(rest, 0))?;
            let dest = positional(rest, 1).ok_or("収集先のフォルダを指定してください")?;
            let comp = opt(rest, "--comp").map(CompId::new);
            let r = collect_files(&project, &base_dir(&path), comp.as_ref(), Path::new(dest)).map_err(|e| e.to_string())?;
            println!("{} に {} 個の素材（{:.1} MB）を集めました", dest, r.copied, r.total_bytes as f64 / 1e6);
            for m in &r.missing {
                println!("警告: 元ファイルが無いためコピーできませんでした: {m}");
            }
            Ok(())
        }
        "relink" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let search = positional(rest, 1).ok_or("探すフォルダを指定してください")?;
            let n = relink(&mut project, &base_dir(&path), Path::new(search));
            save(&project, &path)?;
            let left = missing_footage(&project, &base_dir(&path)).len();
            println!("{n} 個をつなぎ直しました（残り {left} 個）");
            Ok(())
        }
        "beats" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let comp_id = pick_comp(&project, rest)?.id.clone();
            let layer = LayerId::new(opt(rest, "--layer").ok_or("--layer で音声のあるレイヤーを指定してください")?);
            let sensitivity: f64 = opt_num(rest, "--sensitivity", 0.5)?;
            let clips = FfmpegClips::new(base_dir(&path));
            let onsets = {
                let mixer = Mixer::new(&project, &clips);
                let comp = project.comp(&comp_id).unwrap();
                let stereo = mixer.mix_layer(&comp_id, &layer, 0, mixer.comp_frames(comp))?;
                let mono: Vec<f32> = stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
                detect_onsets(&mono, sensitivity)
            };
            let comp = project.comp_mut(&comp_id).unwrap();
            let fr = comp.frame_rate;
            for (i, t) in onsets.iter().enumerate() {
                // マーカーは最寄りのフレームに置く
                comp.markers.push(Marker {
                    time: beat::nearest_frame(Time::from_seconds(*t), fr),
                    duration: Time::ZERO,
                    comment: format!("ビート {}", i + 1),
                });
            }
            comp.markers.sort_by_key(|m| m.time);
            comp.markers.dedup_by_key(|m| m.time);
            save(&project, &path)?;
            println!("{} 個のビートマーカーを打ちました", onsets.len());
            Ok(())
        }
        "band-keys" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let comp_id = pick_comp(&project, rest)?.id.clone();
            let layer = LayerId::new(opt(rest, "--layer").ok_or("--layer で音声のあるレイヤーを指定してください")?);
            let target = LayerId::new(opt(rest, "--target").ok_or("--target でキーフレームを打つレイヤーを指定してください")?);
            let band = match opt(rest, "--band").unwrap_or("low") {
                "low" => Band::LOW,
                "mid" => Band::MID,
                "high" => Band::HIGH,
                "full" => Band::FULL,
                other => return Err(format!("帯域「{other}」は未対応です（low / mid / high / full）")),
            };
            let clips = FfmpegClips::new(base_dir(&path));
            let keys = {
                let mixer = Mixer::new(&project, &clips);
                let comp = project.comp(&comp_id).unwrap();
                let stereo = mixer.mix_layer(&comp_id, &layer, 0, mixer.comp_frames(comp))?;
                let mono: Vec<f32> = stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
                band_keyframes(&mono, band, comp.frame_rate, Time::ZERO, comp.duration)
            };
            let n = keys.keyframes.len();
            let l = project.comp_mut(&comp_id).and_then(|c| c.layer_mut(&target)).ok_or("対象レイヤーが見つかりません")?;
            // 振幅 0..1 → 不透明度 0..100（振り切れは 100 で頭打ち）
            l.transform.opacity = Property::constant(0.0).with_keys(
                keys.keyframes
                    .into_iter()
                    .map(|mut k| {
                        k.value = (k.value * 2.0).min(100.0);
                        k
                    })
                    .collect(),
            );
            save(&project, &path)?;
            println!("{n} 個のキーフレームを「{target}」の不透明度に打ちました");
            Ok(())
        }
        "lyrics" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let file = positional(rest, 1).ok_or("歌詞ファイルを指定してください")?;
            let comp_id = pick_comp(&project, rest)?.id.clone();
            let src = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let lines = lyrics::parse_lyrics(file, &src)?;
            let index = project.compositions.iter().position(|c| c.id == comp_id).unwrap();
            let n = lyrics::add_lyric_layers(&mut project, index, &lines, opt_num(rest, "--size", 72.0)?);
            save(&project, &path)?;
            println!("{} 行中 {n} 行をテキストレイヤーにしました", lines.len());
            Ok(())
        }
        "snapshot" => {
            let (project, path) = load(positional(rest, 0))?;
            match positional(rest, 1) {
                Some(name) => {
                    let p = io::save_snapshot(&project, &path, name).map_err(|e| e.to_string())?;
                    println!("版「{name}」を保存しました: {}", p.display());
                }
                None => {
                    for (name, secs, p) in io::list_snapshots(&path) {
                        println!("{secs}  {name}  {}", p.display());
                    }
                }
            }
            Ok(())
        }
        "diff" => {
            let (a, pa) = load(positional(rest, 0))?;
            let (b, _) = load(positional(rest, 1))?;
            let comp = pick_comp(&a, rest)?;
            let footage = FfmpegFootage::new(base_dir(&pa));
            match opt(rest, "--frame") {
                Some(_) => {
                    let frame: i64 = opt_num(rest, "--frame", 0)?;
                    let d = version_diff(&a, &b, &footage, &comp.id, comp.frame_rate.frame_to_time(frame), 1).map_err(|e| e.to_string())?;
                    match d.bbox {
                        Some(r) => println!(
                            "フレーム {frame}: {} 画素が違います（範囲 x {}..{}, y {}..{}）",
                            d.changed_pixels, r[0], r[2], r[1], r[3]
                        ),
                        None => println!("フレーム {frame}: 違いはありません"),
                    }
                    if let Some(out) = opt(rest, "-o") {
                        save_png(&d.heatmap, Path::new(out), None).map_err(|e| e.to_string())?;
                        println!("差分画像: {out}");
                    }
                }
                None => {
                    let frames = changed_frames(&a, &b, &footage, &comp.id, 1).map_err(|e| e.to_string())?;
                    if frames.is_empty() {
                        println!("違いはありません");
                    } else {
                        // 連続するフレームを範囲にまとめる
                        let mut ranges: Vec<(i64, i64)> = Vec::new();
                        for f in frames {
                            match ranges.last_mut() {
                                Some(r) if r.1 + 1 == f => r.1 = f,
                                _ => ranges.push((f, f)),
                            }
                        }
                        println!(
                            "絵が変わったフレーム: {}",
                            ranges
                                .iter()
                                .map(|(a, b)| if a == b { a.to_string() } else { format!("{a}–{b}") })
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
            }
            Ok(())
        }
        "export-comp" => {
            let (project, _) = load(positional(rest, 0))?;
            let id = CompId::new(positional(rest, 1).ok_or("コンポ ID を指定してください")?.as_str());
            let out = opt(rest, "-o").ok_or("-o で出力ファイルを指定してください")?;
            let pkg = collab::export_comp(&project, &id).ok_or_else(|| format!("コンポ「{id}」が見つかりません"))?;
            save(&pkg, Path::new(out))?;
            println!("{out} に {} 個のコンポを書き出しました", pkg.compositions.len());
            Ok(())
        }
        "import-comp" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let (pkg, _) = load(positional(rest, 1))?;
            let id = collab::import_comp(&mut project, &pkg).ok_or("パッケージにコンポがありません")?;
            save(&project, &path)?;
            println!("コンポ「{id}」として読み込みました");
            Ok(())
        }
        "breakdown" => {
            let (project, path) = load(positional(rest, 0))?;
            let comp = pick_comp(&project, rest)?;
            let out = opt(rest, "-o").ok_or("-o で出力フォルダを指定してください")?;
            let frame: i64 = opt_num(rest, "--frame", 0)?;
            let dir = base_dir(&path);
            let footage = FfmpegFootage::new(&dir);
            let engine = ExprEngine::new();
            let ctx = ExportContext {
                project: &project,
                footage: &footage,
                clips: None,
                resolver: Some(&engine),
                base_dir: Some(dir),
                accel: gpu(rest),
                composite: gpu_composite(rest),
            };
            let files = export_breakdown(&ctx, &comp.id, comp.frame_rate.frame_to_time(frame), Path::new(out), "breakdown")
                .map_err(|e| e.to_string())?;
            println!("{} 枚を {out} に書き出しました", files.len());
            Ok(())
        }
        "stats" => {
            let (project, path) = load(positional(rest, 0))?;
            print!("{}", log::summary_text(&project, &log::read_sessions(&path)));
            Ok(())
        }
        "script" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let file = positional(rest, 1).ok_or("スクリプトファイルを指定してください")?;
            let src = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            for line in run_script(&mut project, &src)? {
                println!("{line}");
            }
            save(&project, &path)?;
            println!("{} を更新しました", path.display());
            Ok(())
        }
        "import" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let file = positional(rest, 1).ok_or("読み込むファイルを指定してください")?;
            let comp_id = pick_comp(&project, rest)?.id.clone();
            let mut opts = ImportOptions { sequence: flag(rest, "--sequence"), ..Default::default() };
            if let Some(fps) = opt(rest, "--fps") {
                let v: f64 = fps.parse().map_err(|_| format!("--fps の値「{fps}」が不正です"))?;
                opts.sequence_rate = FrameRate { num: (v * 1000.0).round() as u32, den: 1000 };
            }
            let out = import_file(&mut project, &comp_id, Path::new(file), opts).map_err(|e| e.to_string())?;
            save(&project, &path)?;
            println!("{}", out.summary);
            for w in &out.warnings {
                println!("  注意: {w}");
            }
            Ok(())
        }
        "camera-import" => {
            let (mut project, path) = load(positional(rest, 0))?;
            let file = positional(rest, 1).ok_or(".chan ファイルを指定してください")?;
            let comp_id = pick_comp(&project, rest)?.id.clone();
            let id = project.fresh_id("camera");
            let comp = project.comp_mut(&comp_id).unwrap();
            let scale = opt_num(rest, "--scale", 100.0)?;
            let mut cam = if file.to_lowercase().ends_with(".abc") {
                abc_camera_layer(Path::new(file), comp.width, comp.height, comp.frame_rate, scale).map_err(|e| e.to_string())?
            } else {
                let src = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
                camera_import::parse_chan(&src, comp.width, comp.height, comp.frame_rate, scale, 39.6)?
            };
            cam.id = LayerId::new(id);
            cam.out_point = cam.out_point.max(comp.duration);
            comp.layers.insert(0, cam);
            save(&project, &path)?;
            println!("カメラを読み込みました");
            Ok(())
        }
        "proxy" => {
            let (project, path) = load(positional(rest, 0))?;
            let dir = base_dir(&path);
            let scale: u32 = opt_num(rest, "--scale", 2)?;
            let videos: Vec<&FootageItem> = project.footage.iter().filter(|f| f.frame_rate.is_some() && f.width > 0).collect();
            let results: Vec<Result<String, String>> = videos
                .par_iter()
                .map(|f| {
                    make_proxy(&dir, f, scale)
                        .map(|(p, w, h)| format!("{} → {} ({w}x{h})", f.name, p.display()))
                        .map_err(|e| format!("{}: {e}", f.name))
                })
                .collect();
            for r in results {
                match r {
                    Ok(m) => println!("{m}"),
                    Err(e) => eprintln!("失敗: {e}"),
                }
            }
            Ok(())
        }
        "bench" => bench(
            opt_num(rest, "--layers", 200)?,
            opt_num(rest, "--scale", 2)?,
            opt_num(rest, "--frames", 30)?,
            opt_num(rest, "--effects", 0)?,
            flag(rest, "--full"),
            gpu(rest),
            gpu_composite(rest),
        ),
        "plugin-scan" => {
            let file = positional(rest, 0).ok_or("スキャンするファイルかフォルダを指定してください")?;
            let path = Path::new(file);
            let declared = if plugins::package::is_package(path) {
                let m = plugins::package::read_manifest(path)?;
                plugins::package::declared(path, &m)
            } else {
                Default::default()
            };
            let r = plugins::scan::scan_path(path, &declared);
            print_report(&r, true);
            println!("\n{}", plugins::DISCLAIMER);
            Ok(())
        }
        "plugins" => {
            if let Some(file) = positional(rest, 0) {
                // 指定したファイルだけを試しに読み込む（プラグイン開発時の確認用）
                let infos = plugins::load_plugin_file(Path::new(file))?;
                println!("{file}: エフェクト {} 個", infos.len());
            } else {
                println!("探す場所:");
                for d in plugins::plugin_dirs() {
                    println!("  {}{}", d.display(), if d.is_dir() { "" } else { "（なし）" });
                }
                for (file, e) in plugins::scan_default().errors {
                    println!("読み込めません: {}: {e}", file.display());
                }
            }
            for (path, r) in plugins::blocked() {
                println!("\n読み込みを止めました: {}", path.display());
                print_report(&r, false);
                println!("  内容を確認し、自己責任で使う場合はアプリの「プラグイン」ウィンドウで許可してください");
            }
            for pkg in plugins::packages().iter().filter(|p| p.status != plugins::PackageStatus::Blocked) {
                let m = &pkg.manifest;
                println!("\nパッケージ {} 「{}」 {}", m.id, m.name, m.version);
                println!("  フォルダ: {}", pkg.dir.display());
                println!("  {}", pkg.report.summary());
                println!(
                    "  コマンド {} / パネル {} / 出力フィルター {} / エクスプレッション用ライブラリ {} / エフェクト {}",
                    pkg.commands.len(),
                    pkg.panels.len(),
                    m.output_filters.len(),
                    m.expressions.len(),
                    pkg.effect_count
                );
                if let plugins::PackageStatus::Failed(e) = &pkg.status {
                    println!("  読み込めません: {e}");
                }
                for w in &pkg.warnings {
                    println!("  注意: {w}");
                }
            }
            let list = plugins::loaded();
            if list.is_empty() {
                println!("エフェクトのプラグインは読み込まれていません");
            }
            for i in list {
                let def = effect::effect_def(&i.kind);
                println!("\n{} 「{}」 [{} / {}]", i.kind, i.label, i.api.label(), i.category);
                println!("  ファイル: {}", i.path.display());
                let or_unknown = |s: &str| if s.is_empty() { "（記載なし）".to_string() } else { s.to_string() };
                println!("  提供元: {} / 版: {} / ライセンス: {}", or_unknown(&i.vendor), or_unknown(&i.version), or_unknown(&i.license));
                if let Some(r) = i.report() {
                    println!("  {}", r.summary());
                }
                for p in def.map(|d| d.params).unwrap_or_default() {
                    println!("  - {}（{}）", p.name, p.label);
                }
            }
            Ok(())
        }
        "licenses" => {
            for c in licenses::COMPONENTS {
                println!("{}\n  ライセンス: {}\n  用途: {}", c.name, c.license, c.usage);
            }
            println!("\n{}", licenses::NOTICE);
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("不明なコマンド「{other}」\n\n{USAGE}")),
    }
}

fn print_report(r: &plugins::SecurityReport, full: bool) {
    println!("  {}", r.summary());
    println!("  調べたファイル: {} 個 / SHA-256: {}", r.files_scanned, r.hash);
    for f in &r.findings {
        println!("  - [{}] +{} {}", f.category.label(), f.score, f.title);
        if full && !f.evidence.is_empty() {
            println!("      根拠: {}", f.evidence.join(", "));
        }
        if full && f.file != r.path {
            println!("      ファイル: {}", f.file.display());
        }
    }
    if full && !r.endpoints.is_empty() {
        println!("  見つかった通信先: {}", r.endpoints.join(", "));
    }
}

/// 1080p・`layers` 枚（半透明・回転・キーフレーム付き、描画モード混在）のコンポを作って合成速度を測る。
fn bench(
    layers: usize,
    scale: u32,
    frames: i64,
    effects: usize,
    full: bool,
    accel: Option<&'static dyn znd_render::effects::EffectAccel>,
    composite: Option<&'static dyn znd_render::gpu_composite::CompositeAccel>,
) -> Result<(), String> {
    let dur = Time::from_seconds(10.0);
    let modes = [BlendMode::Normal, BlendMode::Add, BlendMode::Screen, BlendMode::Multiply, BlendMode::Overlay];
    let mut list = Vec::with_capacity(layers);
    for i in 0..layers {
        // 決定的な擬似乱数で配置をばらけさせる
        let r = |k: u64| ((i as u64 * 2654435761 + k * 40503) % 1000) as f64 / 1000.0;
        // --full: すべてのレイヤーを画面いっぱいの大きさにする（画面全体に掛かるエフェクトの負荷を測る）
        let (w, h) = if full { (1920, 1080) } else { (200 + (r(1) * 500.0) as u32, 120 + (r(2) * 400.0) as u32) };
        let mut l = Layer::new(
            format!("l{i}"),
            format!("レイヤー {i}"),
            LayerKind::Solid { width: w, height: h, color: [r(3) as f32, r(4) as f32, r(5) as f32] },
            Transform::centered(w as f64, h as f64, 1920.0, 1080.0),
            dur,
        );
        l.transform.position = Property::spatial([0.0, 0.0]).with_keys(vec![
            Keyframe::easy_ease(Time::ZERO, [r(6) * 1920.0, r(7) * 1080.0]),
            Keyframe::easy_ease(dur, [r(8) * 1920.0, r(9) * 1080.0]),
        ]);
        l.transform.rotation =
            Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, r(10) * 360.0), Keyframe::linear(dur, r(11) * 720.0)]);
        l.transform.opacity = Property::constant(30.0 + r(12) * 60.0);
        l.blend_mode = modes[i % modes.len()];
        // エフェクト多段: グロー → ブラー → レベル補正 → ドロップシャドウ → 色相 … を順に重ねる
        for k in 0..effects {
            let fx = match (i + k) % 5 {
                0 => EffectInstance::new("glow", w as f64, h as f64).map(|e| e.set_f("radius", 30.0).set_f("threshold", 0.3)),
                1 => EffectInstance::new("gaussian_blur", w as f64, h as f64).map(|e| e.set_f("radius", 12.0)),
                2 => EffectInstance::new("levels", w as f64, h as f64).map(|e| e.set_f("gamma", 1.2)),
                3 => EffectInstance::new("drop_shadow", w as f64, h as f64).map(|e| e.set_f("softness", 16.0)),
                _ => EffectInstance::new("hue_saturation", w as f64, h as f64).map(|e| e.set_f("hue", 20.0)),
            };
            l.effects.extend(fx);
        }
        list.push(l);
    }
    let mut project = Project::new("bench");
    let fr = FrameRate::FPS_30;
    project.compositions.push(Composition { layers: list, ..Composition::new("bench", "bench", 1920, 1080, fr, dur) });
    let renderer = Renderer::new(&project, &NoFootage).with_accel(accel).with_composite(composite);
    let opts = RenderOptions { downsample: scale, ..RenderOptions::PREVIEW };
    let id = CompId::new("bench");
    let time_of = |f: i64| fr.frame_to_time(f * 3 % 300);

    let start = Instant::now();
    for f in 0..frames {
        renderer.render_frame(&id, time_of(f), opts).map_err(|e| e.to_string())?;
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    println!(
        "1080p / レイヤー {layers} 枚 / エフェクト各 {effects} 個 / 解像度 1/{scale} / エフェクト・合成: {}",
        if accel.is_some() { "GPU" } else { "CPU" }
    );
    println!("  1 フレームずつ（スクラブ時）      : {ms:6.1} ms/フレーム（{:.1} fps）", 1000.0 / ms);

    // RAM プレビューと書き出しはフレーム単位で並列にレンダリングする
    let threads = rayon::current_num_threads() as i64;
    let start = Instant::now();
    let results: Vec<_> = (0..frames * threads).into_par_iter().map(|f| renderer.render_frame(&id, time_of(f), opts).map(|_| ())).collect();
    results.into_iter().collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    let ms = start.elapsed().as_secs_f64() * 1000.0 / (frames * threads) as f64;
    println!("  複数フレーム並列（RAM プレビュー時）: {ms:6.1} ms/フレーム（{:.1} fps, {threads} スレッド）", 1000.0 / ms);
    Ok(())
}
