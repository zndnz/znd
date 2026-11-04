//! 歌詞・字幕ファイル（LRC / SRT / ASS）の読み込みと、テキストレイヤー・マーカーの自動生成。

use crate::model::{Composition, Layer, LayerKind, Marker, Project, Transform};
use crate::text::{Justify, TextDocument};
use crate::time::Time;

#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    pub start: Time,
    /// LRC のように終了時刻が無い形式では、次の行の開始（最終行は `None`）。
    pub end: Option<Time>,
    pub text: String,
}

fn ms(total_ms: f64) -> Time {
    Time::from_seconds(total_ms / 1000.0)
}

/// `mm:ss.xx` / `h:mm:ss.xx` / `hh:mm:ss,mmm` を解釈する。
fn parse_clock(s: &str) -> Option<Time> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let nums: Option<Vec<f64>> = parts.iter().map(|p| p.trim().parse::<f64>().ok()).collect();
    let nums = nums?;
    let secs = match nums.as_slice() {
        [m, s] => m * 60.0 + s,
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        _ => return None,
    };
    Some(ms(secs * 1000.0))
}

pub fn parse_lrc(src: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in src.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(close) = stripped.find(']') else { break };
            match parse_clock(&stripped[..close]) {
                Some(t) => stamps.push(t),
                // [ar:...] などのメタデータ行
                None => {
                    stamps.clear();
                    rest = "";
                    break;
                }
            }
            rest = stripped[close + 1..].trim_start();
        }
        for t in stamps {
            lines.push(LyricLine { start: t, end: None, text: rest.to_string() });
        }
    }
    lines.sort_by_key(|l| l.start);
    for i in 0..lines.len() {
        lines[i].end = lines.get(i + 1).map(|n| n.start);
    }
    // 空行は「前の行を消すタイミング」なので、終了時刻として使ったあと取り除く
    lines.retain(|l| !l.text.is_empty());
    lines
}

pub fn parse_srt(src: &str) -> Vec<LyricLine> {
    let normalized = src.replace("\r\n", "\n");
    let mut lines = Vec::new();
    for block in normalized.split("\n\n") {
        let mut it = block.lines().filter(|l| !l.trim().is_empty()).peekable();
        // 連番行は省略されることがある
        if it.peek().is_some_and(|l| !l.contains("-->")) {
            it.next();
        }
        let Some(timing) = it.next() else { continue };
        let Some((a, b)) = timing.split_once("-->") else { continue };
        let (Some(start), Some(end)) = (parse_clock(a), parse_clock(b.split_whitespace().next().unwrap_or(""))) else { continue };
        let text = it.collect::<Vec<_>>().join("\n");
        if !text.is_empty() {
            lines.push(LyricLine { start, end: Some(end), text });
        }
    }
    lines
}

pub fn parse_ass(src: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    // Format 行で列順が決まる。無ければ標準の並びを仮定する。
    let mut format: Vec<String> = ["Layer", "Start", "End", "Style", "Name", "MarginL", "MarginR", "MarginV", "Effect", "Text"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut in_events = false;
    for raw in src.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(f) = line.strip_prefix("Format:") {
            format = f.split(',').map(|s| s.trim().to_string()).collect();
        } else if let Some(d) = line.strip_prefix("Dialogue:") {
            // Text 列はカンマを含みうるので、列数ぶんだけ分割する
            let cols: Vec<&str> = d.splitn(format.len(), ',').collect();
            let col = |name: &str| format.iter().position(|f| f == name).and_then(|i| cols.get(i)).copied();
            let (Some(start), Some(end), Some(text)) = (col("Start").and_then(parse_clock), col("End").and_then(parse_clock), col("Text"))
            else {
                continue;
            };
            // {\...} の上書きタグを除き、改行コードを展開する
            let mut clean = String::new();
            let mut depth = 0;
            for ch in text.chars() {
                match ch {
                    '{' => depth += 1,
                    '}' if depth > 0 => depth -= 1,
                    _ if depth == 0 => clean.push(ch),
                    _ => {}
                }
            }
            let clean = clean.replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ");
            if !clean.trim().is_empty() {
                lines.push(LyricLine { start, end: Some(end), text: clean.trim().to_string() });
            }
        }
    }
    lines.sort_by_key(|l| l.start);
    lines
}

/// 拡張子で形式を判定して読み込む。
pub fn parse_lyrics(file_name: &str, src: &str) -> Result<Vec<LyricLine>, String> {
    let ext = file_name.rsplit('.').next().unwrap_or("").to_lowercase();
    let lines = match ext.as_str() {
        "lrc" => parse_lrc(src),
        "srt" => parse_srt(src),
        "ass" | "ssa" => parse_ass(src),
        other => return Err(format!("未対応の歌詞形式です: .{other}（LRC / SRT / ASS に対応）")),
    };
    if lines.is_empty() {
        return Err("歌詞の行が見つかりませんでした".into());
    }
    Ok(lines)
}

/// 歌詞の各行をテキストレイヤーにし、コンポマーカーを置く。開始・終了は最寄りのフレームに丸める。
/// 追加したレイヤー数を返す。
pub fn add_lyric_layers(project: &mut Project, comp_index: usize, lines: &[LyricLine], font_size: f64) -> usize {
    let mut ids: Vec<String> = Vec::new();
    for _ in lines {
        // fresh_id は既存 ID だけを見るので、今回ぶんは自前で重複を避ける
        let mut n = ids.len() + 1;
        let id = loop {
            let candidate = format!("lyric-{n}");
            let used = ids.contains(&candidate) || project.compositions.iter().any(|c| c.layers.iter().any(|l| l.id.0 == candidate));
            if !used {
                break candidate;
            }
            n += 1;
        };
        ids.push(id);
    }
    let comp: &mut Composition = &mut project.compositions[comp_index];
    let fr = comp.frame_rate;
    let mut added = 0;
    for (line, id) in lines.iter().zip(ids) {
        let start = crate::beat::nearest_frame(line.start, fr);
        if start >= comp.duration {
            continue;
        }
        let end = crate::beat::nearest_frame(line.end.unwrap_or(comp.duration), fr).min(comp.duration).max(start + fr.frame_to_time(1));
        let mut doc = TextDocument::new(line.text.clone(), font_size);
        doc.justify = Justify::Center;
        let mut tr = Transform::centered(0.0, 0.0, comp.width as f64, comp.height as f64);
        tr.position.value = [comp.width as f64 / 2.0, comp.height as f64 * 0.8];
        let name: String = line.text.lines().next().unwrap_or("").chars().take(24).collect();
        let mut layer = Layer::new(
            id,
            name,
            LayerKind::Text { document: doc, animators: Vec::new(), path: None, per_char_3d: false, text_expression: None },
            tr,
            comp.duration,
        );
        layer.in_point = start;
        layer.out_point = end;
        comp.layers.insert(0, layer);
        comp.markers.push(Marker { time: start, duration: end - start, comment: line.text.replace('\n', " ") });
        added += 1;
    }
    comp.markers.sort_by_key(|m| m.time);
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: f64) -> Time {
        Time::from_seconds(v)
    }

    #[test]
    fn lrc() {
        let src = "[ti:曲名]\n[00:01.50]一行目\n[00:04.00][00:10.00]くり返し\n[00:06.00]\n";
        let l = parse_lrc(src);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0], LyricLine { start: s(1.5), end: Some(s(4.0)), text: "一行目".into() });
        assert_eq!(l[1].end, Some(s(6.0)));
        assert_eq!(l[2], LyricLine { start: s(10.0), end: None, text: "くり返し".into() });
    }

    #[test]
    fn srt() {
        let src = "1\r\n00:00:01,000 --> 00:00:02,500\r\nこんにちは\r\n世界\r\n\r\n2\r\n00:01:00,250 --> 00:01:01,000 X1:0\r\nつぎ\r\n";
        let l = parse_srt(src);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0], LyricLine { start: s(1.0), end: Some(s(2.5)), text: "こんにちは\n世界".into() });
        assert_eq!(l[1].start, s(60.25));
    }

    #[test]
    fn ass() {
        let src = "[Script Info]\nTitle: x\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
                   Dialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,{\\an8\\fad(100,100)}歌詞, その一\\N二行目\n\
                   Comment: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,無視\n";
        let l = parse_ass(src);
        assert_eq!(l, vec![LyricLine { start: s(1.0), end: Some(s(3.5)), text: "歌詞, その一\n二行目".into() }]);
    }

    #[test]
    fn layers_and_markers_are_created_on_frame_boundaries() {
        let mut p = crate::demo::demo_project();
        let before = p.compositions[0].layers.len();
        let lines = parse_lrc("[00:01.01]あ\n[00:02.52]い\n[00:59.00]尺の外\n");
        let n = add_lyric_layers(&mut p, 0, &lines, 80.0);
        assert_eq!(n, 2);
        let comp = &p.compositions[0];
        assert_eq!(comp.layers.len(), before + 2);
        let fr = comp.frame_rate;
        let first = comp.layers.iter().find(|l| l.name == "あ").unwrap();
        // 1.01 秒は 30fps の最寄りフレーム 30（1.000 秒）に丸まる
        assert_eq!(first.in_point, fr.frame_to_time(30));
        assert_eq!(first.out_point, fr.frame_to_time(76));
        assert!(comp.markers.iter().any(|m| m.comment == "い" && m.time == fr.frame_to_time(76)));
        assert_eq!(p.validate(), Vec::<String>::new());
        assert!(parse_lyrics("a.txt", "").is_err());
    }
}
