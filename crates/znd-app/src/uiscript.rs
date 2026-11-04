//! 画面操作の自動再生（動作確認用）。
//!
//! `znd-app --ui-script 操作.txt` で、ファイルに書いたポインタ・キー操作を 1 行ずつ本物の入力として流し込む。
//! ドラッグなどの操作が実際に効くかを、人の手を借りずに確かめるためのもの。1 行 = 1 フレーム。
//!
//! ```text
//! select LAYER_ID        レイヤーを選択する
//! frame N                再生ヘッドを動かす
//! tool select|pen|rect|ellipse
//! vmove X Y              ビューア上の、コンポ座標 (X, Y) へポインタを動かす
//! tmove LAYER_ID FRAME   タイムラインの、そのレイヤーの行のフレーム位置へ
//! lmove LAYER_ID [DY]    タイムラインの左の、そのレイヤーの名前の上へ（DY は行の中心からの上下のずれ）
//! rmove FRAME            タイムラインのルーラーのフレーム位置へ
//! pmove LAYER_ID PATH FRAME  タイムラインの、そのプロパティの行のフレーム位置へ（レイヤーを展開しておくこと）
//! open LAYER_ID          タイムラインでそのレイヤーを展開する
//! copy / paste           ⌘C / ⌘V（キーフレームのコピー・貼り付け）
//! move X Y               画面座標へ
//! down / up              左ボタンを押す／離す
//! click / dclick         その場でクリック／ダブルクリック
//! rclick                 その場で右クリック
//! mod alt|shift|cmd|none 以後の操作に付ける修飾キー（cmd+shift のように組み合わせ可）
//! key NAME               キーを押して離す（Enter, Escape, Delete, V など）
//! type TEXT              文字を入力する（入力欄にフォーカスがあるとき）
//! scroll DX DY           ホイール
//! wait N                 N フレーム待つ
//! shot PATH              スクリーンショットを保存する
//! save PATH              プロジェクトを保存する
//! quit                   終了する
//! ```

use crate::App;
use eframe::egui::{self, pos2, Event, Key, Modifiers, PointerButton, Pos2};
use std::path::{Path, PathBuf};
use znd_core::{io, LayerId};

pub(crate) struct Script {
    lines: Vec<String>,
    next: usize,
    wait: u32,
    pos: Pos2,
    modifiers: Modifiers,
    /// 次のフレームで流し込む入力。
    pending: Vec<Event>,
}

impl Script {
    pub fn load(path: &Path) -> Option<Script> {
        let text = std::fs::read_to_string(path).map_err(|e| eprintln!("操作スクリプト {}: {e}", path.display())).ok()?;
        let lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
        // 最初の数フレームは画面の配置が落ち着くのを待つ
        Some(Script { lines, next: 0, wait: 20, pos: pos2(0.0, 0.0), modifiers: Modifiers::NONE, pending: Vec::new() })
    }

    /// 溜めてあった入力を、このフレームの入力として流し込む。
    pub fn inject(&mut self, raw: &mut egui::RawInput) {
        raw.modifiers = self.modifiers;
        raw.events.append(&mut self.pending);
    }

    fn button(&mut self, pressed: bool) {
        self.press(PointerButton::Primary, pressed);
    }

    fn press(&mut self, button: PointerButton, pressed: bool) {
        self.pending.push(Event::PointerButton { pos: self.pos, button, pressed, modifiers: self.modifiers });
    }

    fn move_to(&mut self, p: Pos2) {
        self.pos = p;
        self.pending.push(Event::PointerMoved(p));
    }
}

impl App {
    /// 1 フレームに 1 行ずつ実行する。
    pub(crate) fn tick_script(&mut self, ctx: &egui::Context) {
        let Some(mut script) = self.script.take() else { return };
        ctx.request_repaint();
        if script.wait > 0 {
            script.wait -= 1;
            self.script = Some(script);
            return;
        }
        // 撮影の結果が届くまで次へ進まない
        if self.script_shot.is_some() {
            self.script = Some(script);
            return;
        }
        let Some(line) = script.lines.get(script.next).cloned() else {
            // 最後まで実行した。ウィンドウは開いたままにする
            script.modifiers = Modifiers::NONE;
            return;
        };
        script.next += 1;
        let mut it = line.split_whitespace();
        let cmd = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        let num = |i: usize| args.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
        let fail = |what: &str| eprintln!("操作スクリプト: 「{line}」— {what}");
        match cmd {
            "select" => self.selected = args.first().map(|s| LayerId::new(*s)),
            "frame" => self.set_frame(num(0) as i64, false),
            "tool" => {
                self.tool = match args.first().copied() {
                    Some("pen") => crate::tools::Tool::Pen,
                    Some("rect") => crate::tools::Tool::RectMask,
                    Some("ellipse") => crate::tools::Tool::EllipseMask,
                    _ => crate::tools::Tool::Select,
                }
            }
            "move" => script.move_to(pos2(num(0), num(1))),
            "vmove" => match (self.anchors.get("viewer"), self.shared.comp(&self.comp_id)) {
                (Some(r), Some(comp)) => {
                    let scale = r.width() / comp.width as f32;
                    script.move_to(pos2(r.left() + num(0) * scale, r.top() + num(1) * scale));
                }
                _ => fail("ビューアがまだ表示されていません"),
            },
            "tmove" => match (args.first().and_then(|id| self.anchors.get(&format!("row:{id}"))), self.tl_view) {
                (Some(r), Some(view)) => script.move_to(pos2(view.x(num(1) as f64), r.center().y)),
                _ => fail("そのレイヤーの行がタイムラインにありません"),
            },
            "lmove" => match args.first().and_then(|id| self.anchors.get(&format!("row:name:{id}"))) {
                Some(r) => script.move_to(pos2(r.left() + 20.0, r.center().y + num(1))),
                None => fail("そのレイヤーの行がタイムラインにありません"),
            },
            "pmove" => match (args.get(1).and_then(|p| self.anchors.get(&format!("row:prop:{}:{p}", args[0]))), self.tl_view) {
                (Some(r), Some(view)) => script.move_to(pos2(view.x(num(2) as f64), r.center().y)),
                _ => fail("そのプロパティの行がタイムラインにありません"),
            },
            "open" => {
                if let Some(id) = args.first() {
                    if !self.tl_is_open(id, false) {
                        self.tl_open.insert(id.to_string());
                    }
                }
            }
            "copy" => script.pending.push(Event::Copy),
            "paste" => script.pending.push(Event::Paste(String::new())),
            "rmove" => match (self.anchors.get("ruler"), self.tl_view) {
                (Some(r), Some(view)) => script.move_to(pos2(view.x(num(0) as f64), r.center().y)),
                _ => fail("タイムラインがまだ表示されていません"),
            },
            "down" => script.button(true),
            "up" => script.button(false),
            "click" => {
                script.button(true);
                script.button(false);
            }
            "rclick" => {
                script.press(PointerButton::Secondary, true);
                script.press(PointerButton::Secondary, false);
            }
            "dclick" => {
                for _ in 0..2 {
                    script.button(true);
                    script.button(false);
                }
            }
            "mod" => {
                // "cmd+shift" のように組み合わせられる
                let mut m = Modifiers::NONE;
                for part in args.first().copied().unwrap_or("none").split('+') {
                    m = m | match part {
                        "alt" => Modifiers::ALT,
                        "shift" => Modifiers::SHIFT,
                        "cmd" => Modifiers::COMMAND | if cfg!(target_os = "macos") { Modifiers::MAC_CMD } else { Modifiers::CTRL },
                        _ => Modifiers::NONE,
                    };
                }
                script.modifiers = m;
            }
            "key" => match args.first().and_then(|n| Key::from_name(n)) {
                Some(key) => {
                    for pressed in [true, false] {
                        script.pending.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: script.modifiers });
                    }
                }
                None => fail("キーの名前が分かりません"),
            },
            "type" => {
                let text = line.splitn(2, char::is_whitespace).nth(1).unwrap_or("").to_string();
                script.pending.push(Event::Text(text));
            }
            "scroll" => script.pending.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(num(0), num(1)),
                modifiers: script.modifiers,
            }),
            "wait" => script.wait = num(0) as u32,
            "shot" => match args.first() {
                Some(p) => {
                    self.script_shot = Some(PathBuf::from(p));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                }
                None => fail("保存先がありません"),
            },
            "save" => match args.first() {
                Some(p) => {
                    if let Err(e) = io::save(&self.project, Path::new(p)) {
                        fail(&e.to_string());
                    }
                }
                None => fail("保存先がありません"),
            },
            "quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            other => fail(&format!("知らない命令です: {other}")),
        }
        self.script = Some(script);
    }
}
