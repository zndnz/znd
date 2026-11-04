//! キーボードショートカットの割り当て（キーマップ）。
//!
//! 操作（[`Action`]）ごとに 0 個以上のキーの組み合わせを持つ。プリセット（標準 / After Effects 経験者向け）から始めて、
//! 「キーボードショートカット」ウィンドウで変更でき、設定フォルダの `keymap.json` に保存される。
//!
//! ```json
//! { "preset": "standard", "bindings": { "play": ["Space"], "undo": ["Cmd+Z"] } }
//! ```
//!
//! `bindings` に書いた操作だけがプリセットから置き換わる（書いていない操作はプリセットのまま）。

use eframe::egui::{Event, Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// ショートカットで呼べる操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Action {
    Undo,
    Redo,
    Save,
    Play,
    NextFrame,
    PrevFrame,
    NextFrame10,
    PrevFrame10,
    FirstFrame,
    LastFrame,
    PrevKey,
    NextKey,
    GoInPoint,
    GoOutPoint,
    Marker,
    WorkStart,
    WorkEnd,
    ToolSelect,
    ToolHand,
    ToolZoom,
    ToolRotate,
    ToolPen,
    ToolRect,
    ToolEllipse,
    Delete,
    Duplicate,
    SplitLayer,
    TrimInToPlayhead,
    TrimOutToPlayhead,
    MoveInToPlayhead,
    MoveOutToPlayhead,
    SelectAllKeys,
    EasyEase,
    EaseIn,
    EaseOut,
    LinearKeys,
    HoldKeys,
    ReverseKeys,
    Hold1,
    Hold2,
    Hold3,
    Hold4,
    ToggleSnap,
    TimelineZoomIn,
    TimelineZoomOut,
    ShowShortcuts,
}

pub(crate) struct ActionInfo {
    pub action: Action,
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    /// 文字入力中でも効く（取り消し・保存など）。
    pub global: bool,
    /// キーを押しっぱなしにしたときの連続入力でも実行する。
    pub repeat: bool,
}

const fn a(action: Action, id: &'static str, label: &'static str, group: &'static str) -> ActionInfo {
    ActionInfo { action, id, label, group, global: false, repeat: false }
}

const fn g(action: Action, id: &'static str, label: &'static str, group: &'static str) -> ActionInfo {
    ActionInfo { action, id, label, group, global: true, repeat: false }
}

const fn r(action: Action, id: &'static str, label: &'static str, group: &'static str) -> ActionInfo {
    ActionInfo { action, id, label, group, global: false, repeat: true }
}

use Action::*;

pub(crate) const ACTIONS: &[ActionInfo] = &[
    g(Undo, "undo", "取り消す", "編集"),
    g(Redo, "redo", "やり直す", "編集"),
    g(Save, "save", "保存", "編集"),
    a(Delete, "delete", "選択中のキーフレーム／レイヤーを削除", "編集"),
    a(Duplicate, "duplicate", "レイヤーを複製", "レイヤー"),
    a(SplitLayer, "split_layer", "レイヤーを再生ヘッドで分割", "レイヤー"),
    a(TrimInToPlayhead, "trim_in", "イン点を再生ヘッドに（トリム）", "レイヤー"),
    a(TrimOutToPlayhead, "trim_out", "アウト点を再生ヘッドに（トリム）", "レイヤー"),
    a(MoveInToPlayhead, "move_in", "イン点が再生ヘッドに来るよう移動", "レイヤー"),
    a(MoveOutToPlayhead, "move_out", "アウト点が再生ヘッドに来るよう移動", "レイヤー"),
    a(Hold1, "hold_1", "コマ打ちなし", "レイヤー"),
    a(Hold2, "hold_2", "2 コマ打ち", "レイヤー"),
    a(Hold3, "hold_3", "3 コマ打ち", "レイヤー"),
    a(Hold4, "hold_4", "4 コマ打ち", "レイヤー"),
    a(Play, "play", "再生／停止", "再生・移動"),
    r(NextFrame, "next_frame", "1 フレーム進む", "再生・移動"),
    r(PrevFrame, "prev_frame", "1 フレーム戻る", "再生・移動"),
    r(NextFrame10, "next_frame_10", "10 フレーム進む", "再生・移動"),
    r(PrevFrame10, "prev_frame_10", "10 フレーム戻る", "再生・移動"),
    a(FirstFrame, "first_frame", "先頭へ", "再生・移動"),
    a(LastFrame, "last_frame", "末尾へ", "再生・移動"),
    r(PrevKey, "prev_key", "前のキーフレームへ", "再生・移動"),
    r(NextKey, "next_key", "次のキーフレームへ", "再生・移動"),
    a(GoInPoint, "go_in", "レイヤーのイン点へ", "再生・移動"),
    a(GoOutPoint, "go_out", "レイヤーのアウト点へ", "再生・移動"),
    a(Marker, "marker", "マーカーを打つ（再生中はタップ入力）", "タイムライン"),
    a(WorkStart, "work_start", "ワークエリアの開始", "タイムライン"),
    a(WorkEnd, "work_end", "ワークエリアの終了", "タイムライン"),
    a(ToggleSnap, "toggle_snap", "スナップのオン／オフ", "タイムライン"),
    r(TimelineZoomIn, "tl_zoom_in", "タイムラインを拡大", "タイムライン"),
    r(TimelineZoomOut, "tl_zoom_out", "タイムラインを縮小", "タイムライン"),
    a(SelectAllKeys, "select_all_keys", "選択レイヤーのキーフレームをすべて選択", "キーフレーム"),
    a(EasyEase, "easy_ease", "イージーイーズ", "キーフレーム"),
    a(EaseIn, "ease_in", "イーズイン", "キーフレーム"),
    a(EaseOut, "ease_out", "イーズアウト", "キーフレーム"),
    a(LinearKeys, "linear_keys", "リニア", "キーフレーム"),
    a(HoldKeys, "hold_keys", "停止", "キーフレーム"),
    a(ReverseKeys, "reverse_keys", "時間反転", "キーフレーム"),
    a(ToolSelect, "tool_select", "選択ツール", "ツール"),
    a(ToolHand, "tool_hand", "手のひらツール", "ツール"),
    a(ToolZoom, "tool_zoom", "ズームツール", "ツール"),
    a(ToolRotate, "tool_rotate", "回転ツール", "ツール"),
    a(ToolPen, "tool_pen", "ペンツール", "ツール"),
    a(ToolRect, "tool_rect", "矩形マスク", "ツール"),
    a(ToolEllipse, "tool_ellipse", "楕円マスク", "ツール"),
    a(ShowShortcuts, "show_shortcuts", "キーボードショートカットの一覧", "ヘルプ"),
];

pub(crate) fn info(action: Action) -> &'static ActionInfo {
    ACTIONS.iter().find(|i| i.action == action).expect("すべての操作に情報がある")
}

fn by_id(id: &str) -> Option<Action> {
    ACTIONS.iter().find(|i| i.id == id).map(|i| i.action)
}

/// キーの組み合わせ 1 つ。`command` は macOS では ⌘、それ以外では Ctrl。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Chord {
    pub command: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Chord {
    pub fn parse(s: &str) -> Option<Chord> {
        let mut c = Chord { command: false, ctrl: false, alt: false, shift: false, key: Key::Escape };
        let mut key = None;
        // "Cmd++" のように、キーそのものが + の場合に備えて末尾から読む
        let (mods, last) = match s.trim().rsplit_once('+') {
            Some((m, "")) => (m.strip_suffix('+').unwrap_or(m), "+"),
            Some((m, k)) => (m, k),
            None => ("", s.trim()),
        };
        for part in mods.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "mod" => c.command = true,
                "ctrl" | "control" => c.ctrl = true,
                "alt" | "option" | "opt" => c.alt = true,
                "shift" => c.shift = true,
                _ => return None,
            }
        }
        let last = last.trim();
        for name in [last.to_string(), last.to_ascii_uppercase(), capitalize(last)] {
            key = key.or_else(|| Key::from_name(&name));
        }
        c.key = key?;
        Some(c)
    }

    /// 設定ファイルに書く表記（"Cmd+Shift+Z"）。
    pub fn to_config(self) -> String {
        let mut s = String::new();
        for (on, name) in [(self.command, "Cmd+"), (self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+")] {
            if on {
                s.push_str(name);
            }
        }
        s.push_str(self.key.name());
        s
    }

    /// 画面に出す表記（macOS では Shift+⌘Z）。
    pub fn display(self) -> String {
        if cfg!(target_os = "macos") {
            let mut s = String::new();
            // ⌥ ⇧ ⌃ は日本語フォントに無いことがあるので文字で書く（⌘ はある）
            for (on, sym) in [(self.ctrl, "Ctrl+"), (self.alt, "Opt+"), (self.shift, "Shift+"), (self.command, "⌘")] {
                if on {
                    s.push_str(sym);
                }
            }
            s + &key_label(self.key)
        } else {
            self.to_config().replace("Cmd+", "Ctrl+")
        }
    }

    /// 押されたキーの修飾と完全に一致するか（余計な修飾キーが押されていれば一致しない）。
    pub fn matches(self, key: Key, m: Modifiers) -> bool {
        let ctrl_extra = if cfg!(target_os = "macos") { m.ctrl } else { false };
        key == self.key && m.command == self.command && m.alt == self.alt && m.shift == self.shift && ctrl_extra == self.ctrl
    }

    pub fn from_event(key: Key, m: Modifiers) -> Chord {
        Chord { command: m.command, ctrl: cfg!(target_os = "macos") && m.ctrl, alt: m.alt, shift: m.shift, key }
    }

    fn modifier_count(self) -> usize {
        [self.command, self.ctrl, self.alt, self.shift].iter().filter(|b| **b).count()
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
}

fn key_label(k: Key) -> String {
    match k {
        Key::ArrowLeft => "←".into(),
        Key::ArrowRight => "→".into(),
        Key::ArrowUp => "↑".into(),
        Key::ArrowDown => "↓".into(),
        Key::Space => "Space".into(),
        Key::OpenBracket => "[".into(),
        Key::CloseBracket => "]".into(),
        Key::Minus => "-".into(),
        Key::Equals => "=".into(),
        Key::Plus => "+".into(),
        Key::Comma => ",".into(),
        Key::Period => ".".into(),
        Key::Slash => "/".into(),
        Key::Backslash => "\\".into(),
        Key::Semicolon => ";".into(),
        Key::Backtick => "`".into(),
        Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9 => {
            k.symbol_or_name().to_string()
        }
        _ => k.name().to_string(),
    }
}

/// 既定の割り当ての組。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Preset {
    Standard,
    AfterEffects,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Standard, Preset::AfterEffects];

    pub fn id(self) -> &'static str {
        match self {
            Preset::Standard => "standard",
            Preset::AfterEffects => "after_effects",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::Standard => "znd 標準",
            Preset::AfterEffects => "After Effects 経験者向け",
        }
    }

    fn from_id(s: &str) -> Preset {
        Preset::ALL.into_iter().find(|p| p.id() == s).unwrap_or(Preset::Standard)
    }

    fn bindings(self, action: Action) -> &'static [&'static str] {
        // 両方で同じもの
        let common: &[&str] = match action {
            Undo => &["Cmd+Z"],
            Redo => &["Cmd+Shift+Z"],
            Save => &["Cmd+S"],
            Play => &["Space"],
            Delete => &["Delete", "Backspace"],
            Duplicate => &["Cmd+D"],
            SplitLayer => &["Cmd+Shift+D"],
            FirstFrame => &["Home"],
            LastFrame => &["End"],
            PrevKey => &["J"],
            NextKey => &["K"],
            Marker => &["M"],
            WorkStart => &["B"],
            WorkEnd => &["N"],
            Hold1 => &["Alt+1"],
            Hold2 => &["Alt+2"],
            Hold3 => &["Alt+3"],
            Hold4 => &["Alt+4"],
            EasyEase => &["F9"],
            EaseIn => &["Shift+F9"],
            EaseOut => &["Cmd+Shift+F9"],
            SelectAllKeys => &["Cmd+Alt+A"],
            TimelineZoomIn => &["Equals"],
            TimelineZoomOut => &["Minus"],
            ToolSelect => &["V"],
            ToolHand => &["H"],
            ToolZoom => &["Z"],
            ToolRotate => &["W"],
            ToolPen => &["G"],
            ShowShortcuts => &["Cmd+Slash"],
            _ => &[],
        };
        let specific: Option<&[&str]> = match (self, action) {
            (Preset::Standard, NextFrame) => Some(&["ArrowRight", "PageDown"]),
            (Preset::Standard, PrevFrame) => Some(&["ArrowLeft", "PageUp"]),
            (Preset::Standard, NextFrame10) => Some(&["Shift+ArrowRight"]),
            (Preset::Standard, PrevFrame10) => Some(&["Shift+ArrowLeft"]),
            (Preset::Standard, ToolRect) => Some(&["R"]),
            (Preset::Standard, ToolEllipse) => Some(&["O"]),
            (Preset::Standard, GoInPoint) => Some(&["Alt+I"]),
            (Preset::Standard, GoOutPoint) => Some(&["Alt+O"]),
            (Preset::Standard, TrimInToPlayhead) => Some(&["Alt+OpenBracket"]),
            (Preset::Standard, TrimOutToPlayhead) => Some(&["Alt+CloseBracket"]),
            (Preset::Standard, MoveInToPlayhead) => Some(&["OpenBracket"]),
            (Preset::Standard, MoveOutToPlayhead) => Some(&["CloseBracket"]),
            (Preset::Standard, ToggleSnap) => Some(&["Shift+S"]),
            // After Effects に慣れた人が迷わない配置
            (Preset::AfterEffects, NextFrame) => Some(&["PageDown", "Cmd+ArrowRight"]),
            (Preset::AfterEffects, PrevFrame) => Some(&["PageUp", "Cmd+ArrowLeft"]),
            (Preset::AfterEffects, NextFrame10) => Some(&["Shift+PageDown", "Cmd+Shift+ArrowRight"]),
            (Preset::AfterEffects, PrevFrame10) => Some(&["Shift+PageUp", "Cmd+Shift+ArrowLeft"]),
            (Preset::AfterEffects, GoInPoint) => Some(&["I"]),
            (Preset::AfterEffects, GoOutPoint) => Some(&["O"]),
            (Preset::AfterEffects, ToolRect) => Some(&["Q"]),
            (Preset::AfterEffects, ToolEllipse) => Some(&["Shift+Q"]),
            (Preset::AfterEffects, TrimInToPlayhead) => Some(&["Alt+OpenBracket"]),
            (Preset::AfterEffects, TrimOutToPlayhead) => Some(&["Alt+CloseBracket"]),
            (Preset::AfterEffects, MoveInToPlayhead) => Some(&["OpenBracket"]),
            (Preset::AfterEffects, MoveOutToPlayhead) => Some(&["CloseBracket"]),
            (Preset::AfterEffects, ToggleSnap) => Some(&["Cmd+Shift+Semicolon"]),
            (Preset::AfterEffects, HoldKeys) => Some(&["Cmd+Alt+H"]),
            (Preset::AfterEffects, ShowShortcuts) => Some(&["Cmd+Alt+Slash"]),
            _ => None,
        };
        specific.unwrap_or(common)
    }
}

/// 操作ごとの割り当て。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Keymap {
    pub preset: Preset,
    /// プリセットから変更した操作（空の配列 = 割り当てなし）。
    pub overrides: BTreeMap<Action, Vec<Chord>>,
}

#[derive(Serialize, Deserialize, Default)]
struct KeymapFile {
    #[serde(default)]
    preset: String,
    #[serde(default)]
    bindings: BTreeMap<String, Vec<String>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap { preset: Preset::Standard, overrides: BTreeMap::new() }
    }
}

impl Keymap {
    pub fn preset_chords(preset: Preset, action: Action) -> Vec<Chord> {
        preset.bindings(action).iter().filter_map(|s| Chord::parse(s)).collect()
    }

    pub fn chords(&self, action: Action) -> Vec<Chord> {
        self.overrides.get(&action).cloned().unwrap_or_else(|| Self::preset_chords(self.preset, action))
    }

    pub fn set(&mut self, action: Action, chords: Vec<Chord>) {
        if chords == Self::preset_chords(self.preset, action) {
            self.overrides.remove(&action);
        } else {
            self.overrides.insert(action, chords);
        }
    }

    /// 押されたキーに対応する操作。修飾キーの多い割り当てほど優先する。
    pub fn lookup(&self, key: Key, m: Modifiers) -> Option<Action> {
        ACTIONS
            .iter()
            .flat_map(|i| self.chords(i.action).into_iter().map(move |c| (i.action, c)))
            .filter(|(_, c)| c.matches(key, m))
            .max_by_key(|(_, c)| c.modifier_count())
            .map(|(a, _)| a)
    }

    /// 同じキーが複数の操作に割り当てられているもの。
    pub fn conflicts(&self) -> Vec<(Chord, Vec<Action>)> {
        let mut seen: Vec<(Chord, Vec<Action>)> = Vec::new();
        for i in ACTIONS {
            for c in self.chords(i.action) {
                match seen.iter_mut().find(|(k, _)| *k == c) {
                    Some((_, v)) => v.push(i.action),
                    None => seen.push((c, vec![i.action])),
                }
            }
        }
        seen.into_iter().filter(|(_, v)| v.len() > 1).collect()
    }

    pub fn to_json(&self) -> String {
        let file = KeymapFile {
            preset: self.preset.id().to_string(),
            bindings: self
                .overrides
                .iter()
                .map(|(a, cs)| (info(*a).id.to_string(), cs.iter().map(|c| c.to_config()).collect()))
                .collect(),
        };
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// 読めない行（知らない操作・キー）は飛ばして、警告の一覧と一緒に返す。
    pub fn from_json(s: &str) -> Result<(Keymap, Vec<String>), String> {
        let file: KeymapFile = serde_json::from_str(s).map_err(|e| e.to_string())?;
        let mut km = Keymap { preset: Preset::from_id(&file.preset), overrides: BTreeMap::new() };
        let mut warnings = Vec::new();
        for (id, keys) in file.bindings {
            let Some(action) = by_id(&id) else {
                warnings.push(format!("知らない操作: {id}"));
                continue;
            };
            let mut chords = Vec::new();
            for k in keys {
                match Chord::parse(&k) {
                    Some(c) => chords.push(c),
                    None => warnings.push(format!("{id}: キーが読めません: {k}")),
                }
            }
            km.overrides.insert(action, chords);
        }
        Ok((km, warnings))
    }

    pub fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("keymap.json"))
    }

    pub fn load() -> (Keymap, Vec<String>) {
        let Some(path) = Self::path() else { return (Keymap::default(), Vec::new()) };
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::from_json(&s).unwrap_or_else(|e| (Keymap::default(), vec![format!("{}: {e}", path.display())])),
            Err(_) => (Keymap::default(), Vec::new()),
        }
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = Self::path().ok_or("設定フォルダが分かりません")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, self.to_json()).map_err(|e| e.to_string())?;
        Ok(path)
    }
}

/// 設定フォルダ。`ZND_CONFIG_DIR` があればそこ。
pub(crate) fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("ZND_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support/znd"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("znd"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config"))).map(|d| d.join("znd"))
    }
}

/// このフレームに押されたキーから、実行する操作を取り出す（取り出したキーの入力は消費する）。
/// `typing` が true（文字入力中）なら、文字入力中でも効く操作だけ。
pub(crate) fn take_actions(km: &Keymap, events: &mut Vec<Event>, typing: bool) -> Vec<Action> {
    let mut out = Vec::new();
    events.retain(|e| {
        let Event::Key { key, pressed: true, repeat, modifiers, .. } = e else { return true };
        let Some(action) = km.lookup(*key, *modifiers) else { return true };
        let i = info(action);
        if (typing && !i.global) || (*repeat && !i.repeat) {
            return true;
        }
        out.push(action);
        false
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }

    #[test]
    fn every_preset_binding_parses() {
        for p in Preset::ALL {
            for i in ACTIONS {
                for s in p.bindings(i.action) {
                    assert!(Chord::parse(s).is_some(), "{:?} {} {s}", p, i.id);
                }
            }
        }
    }

    #[test]
    fn presets_have_no_conflicts() {
        for p in Preset::ALL {
            let km = Keymap { preset: p, overrides: BTreeMap::new() };
            assert!(km.conflicts().is_empty(), "{:?}: {:?}", p, km.conflicts());
        }
    }

    #[test]
    fn parse_and_print_roundtrip() {
        for s in ["Cmd+Shift+Z", "Alt+1", "Space", "Shift+F9", "Alt+OpenBracket", "Cmd+Alt+A"] {
            assert_eq!(Chord::parse(s).unwrap().to_config(), s);
        }
        assert_eq!(Chord::parse("cmd+shift+z"), Chord::parse("Cmd+Shift+Z"));
        assert!(Chord::parse("Hyper+Z").is_none());
    }

    #[test]
    fn exact_modifier_matching_prefers_specific() {
        let km = Keymap::default();
        assert_eq!(km.lookup(Key::Z, Modifiers::COMMAND), Some(Undo));
        assert_eq!(km.lookup(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT), Some(Redo));
        assert_eq!(km.lookup(Key::Z, Modifiers::NONE), Some(ToolZoom));
        assert_eq!(km.lookup(Key::ArrowRight, Modifiers::SHIFT), Some(NextFrame10));
        assert_eq!(km.lookup(Key::Num2, Modifiers::ALT), Some(Hold2));
        assert_eq!(km.lookup(Key::Num2, Modifiers::NONE), None);
    }

    #[test]
    fn take_actions_respects_typing_and_consumes() {
        let km = Keymap::default();
        let mut ev = vec![key(Key::Space, Modifiers::NONE), key(Key::Z, Modifiers::COMMAND), Event::Text("a".into())];
        assert_eq!(take_actions(&km, &mut ev, true), vec![Undo]);
        assert_eq!(ev.len(), 2);
        assert_eq!(take_actions(&km, &mut ev, false), vec![Play]);
        assert_eq!(ev.len(), 1);
    }

    #[test]
    fn overrides_roundtrip_through_json() {
        let mut km = Keymap { preset: Preset::AfterEffects, overrides: BTreeMap::new() };
        km.set(Play, vec![Chord::parse("Shift+Space").unwrap()]);
        km.set(Marker, Vec::new());
        km.set(Undo, Keymap::preset_chords(Preset::AfterEffects, Undo));
        let (back, warnings) = Keymap::from_json(&km.to_json()).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(back, km);
        assert_eq!(back.lookup(Key::Space, Modifiers::SHIFT), Some(Play));
        assert_eq!(back.lookup(Key::M, Modifiers::NONE), None);
        assert_eq!(back.overrides.len(), 2);
        let (_, w) = Keymap::from_json(r#"{"preset":"standard","bindings":{"nope":["X"],"play":["Hyper+Q"]}}"#).unwrap();
        assert_eq!(w.len(), 2);
    }
}
