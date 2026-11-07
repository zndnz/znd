// キーボードショートカットの割り当て。znd-app と同じ操作・プリセット・設定ファイル（keymap.json）を使う。
//
// 操作ごとに 0 個以上のキーの組み合わせ（"Cmd+Shift+Z" のような表記）を持つ。プリセット（標準 / After Effects
// 経験者向け）から始めて、`bindings` に書いた操作だけがプリセットから置き換わる。

import { api } from "./api";

export interface ActionInfo {
  id: string;
  label: string;
  group: string;
  /** 文字入力中でも効く。 */
  global?: boolean;
  /** 押しっぱなしの連続入力でも実行する。 */
  repeat?: boolean;
}

const a = (id: string, label: string, group: string, opt: Partial<ActionInfo> = {}): ActionInfo => ({ id, label, group, ...opt });

export const ACTIONS: ActionInfo[] = [
  a("undo", "取り消す", "編集", { global: true }),
  a("redo", "やり直す", "編集", { global: true }),
  a("save", "保存", "編集", { global: true }),
  a("delete", "選択中のキーフレーム／レイヤーを削除", "編集"),
  a("duplicate", "レイヤーを複製", "レイヤー"),
  a("split_layer", "レイヤーを再生ヘッドで分割", "レイヤー"),
  a("trim_in", "イン点を再生ヘッドに（トリム）", "レイヤー"),
  a("trim_out", "アウト点を再生ヘッドに（トリム）", "レイヤー"),
  a("move_in", "イン点が再生ヘッドに来るよう移動", "レイヤー"),
  a("move_out", "アウト点が再生ヘッドに来るよう移動", "レイヤー"),
  a("hold_1", "コマ打ちなし", "レイヤー"),
  a("hold_2", "2 コマ打ち", "レイヤー"),
  a("hold_3", "3 コマ打ち", "レイヤー"),
  a("hold_4", "4 コマ打ち", "レイヤー"),
  a("play", "再生／停止", "再生・移動"),
  a("next_frame", "1 フレーム進む", "再生・移動", { repeat: true }),
  a("prev_frame", "1 フレーム戻る", "再生・移動", { repeat: true }),
  a("next_frame_10", "10 フレーム進む", "再生・移動", { repeat: true }),
  a("prev_frame_10", "10 フレーム戻る", "再生・移動", { repeat: true }),
  a("first_frame", "先頭へ", "再生・移動"),
  a("last_frame", "末尾へ", "再生・移動"),
  a("prev_key", "前のキーフレームへ", "再生・移動", { repeat: true }),
  a("next_key", "次のキーフレームへ", "再生・移動", { repeat: true }),
  a("go_in", "レイヤーのイン点へ", "再生・移動"),
  a("go_out", "レイヤーのアウト点へ", "再生・移動"),
  a("marker", "マーカーを打つ（再生中はタップ入力）", "タイムライン"),
  a("work_start", "ワークエリアの開始", "タイムライン"),
  a("work_end", "ワークエリアの終了", "タイムライン"),
  a("toggle_snap", "スナップのオン／オフ", "タイムライン"),
  a("tl_zoom_in", "タイムラインを拡大", "タイムライン", { repeat: true }),
  a("tl_zoom_out", "タイムラインを縮小", "タイムライン", { repeat: true }),
  a("select_all_keys", "選択レイヤーのキーフレームをすべて選択", "キーフレーム"),
  a("easy_ease", "イージーイーズ", "キーフレーム"),
  a("ease_in", "イーズイン", "キーフレーム"),
  a("ease_out", "イーズアウト", "キーフレーム"),
  a("linear_keys", "リニア", "キーフレーム"),
  a("hold_keys", "停止", "キーフレーム"),
  a("reverse_keys", "時間反転", "キーフレーム"),
  a("apply_ease", "いまのイージングカーブを選択に付ける", "キーフレーム"),
  a("tool_select", "選択ツール", "ツール"),
  a("tool_hand", "手のひらツール", "ツール"),
  a("tool_zoom", "ズームツール", "ツール"),
  a("tool_rotate", "回転ツール", "ツール"),
  a("tool_pen", "ペンツール", "ツール"),
  a("tool_rect", "矩形マスク", "ツール"),
  a("tool_ellipse", "楕円マスク", "ツール"),
  a("show_shortcuts", "キーボードショートカットの一覧", "ヘルプ"),
];

export type Preset = "standard" | "after_effects";
export const PRESETS: [Preset, string][] = [
  ["standard", "znd 標準"],
  ["after_effects", "After Effects 経験者向け"],
];

const COMMON: Record<string, string[]> = {
  undo: ["Cmd+Z"],
  redo: ["Cmd+Shift+Z"],
  save: ["Cmd+S"],
  play: ["Space"],
  delete: ["Delete", "Backspace"],
  duplicate: ["Cmd+D"],
  split_layer: ["Cmd+Shift+D"],
  first_frame: ["Home"],
  last_frame: ["End"],
  prev_key: ["J"],
  next_key: ["K"],
  marker: ["M"],
  work_start: ["B"],
  work_end: ["N"],
  hold_1: ["Alt+1"],
  hold_2: ["Alt+2"],
  hold_3: ["Alt+3"],
  hold_4: ["Alt+4"],
  easy_ease: ["F9"],
  apply_ease: ["E"],
  ease_in: ["Shift+F9"],
  ease_out: ["Cmd+Shift+F9"],
  select_all_keys: ["Cmd+Alt+A"],
  tl_zoom_in: ["Equals"],
  tl_zoom_out: ["Minus"],
  tool_select: ["V"],
  tool_hand: ["H"],
  tool_zoom: ["Z"],
  tool_rotate: ["W"],
  tool_pen: ["G"],
  show_shortcuts: ["Cmd+Slash"],
};

const SPECIFIC: Record<Preset, Record<string, string[]>> = {
  standard: {
    next_frame: ["ArrowRight", "PageDown"],
    prev_frame: ["ArrowLeft", "PageUp"],
    next_frame_10: ["Shift+ArrowRight"],
    prev_frame_10: ["Shift+ArrowLeft"],
    tool_rect: ["R"],
    tool_ellipse: ["O"],
    go_in: ["Alt+I"],
    go_out: ["Alt+O"],
    trim_in: ["Alt+OpenBracket"],
    trim_out: ["Alt+CloseBracket"],
    move_in: ["OpenBracket"],
    move_out: ["CloseBracket"],
    toggle_snap: ["Shift+S"],
  },
  after_effects: {
    next_frame: ["PageDown", "Cmd+ArrowRight"],
    prev_frame: ["PageUp", "Cmd+ArrowLeft"],
    next_frame_10: ["Shift+PageDown", "Cmd+Shift+ArrowRight"],
    prev_frame_10: ["Shift+PageUp", "Cmd+Shift+ArrowLeft"],
    go_in: ["I"],
    go_out: ["O"],
    tool_rect: ["Q"],
    tool_ellipse: ["Shift+Q"],
    trim_in: ["Alt+OpenBracket"],
    trim_out: ["Alt+CloseBracket"],
    move_in: ["OpenBracket"],
    move_out: ["CloseBracket"],
    toggle_snap: ["Cmd+Shift+Semicolon"],
    hold_keys: ["Cmd+Alt+H"],
    show_shortcuts: ["Cmd+Alt+Slash"],
  },
};

export interface Keymap {
  preset: Preset;
  /** プリセットから置き換えた操作。 */
  bindings: Record<string, string[]>;
}

export let keymap: Keymap = { preset: "standard", bindings: {} };
const listeners = new Set<() => void>();
export const onKeymapChange = (f: () => void) => (listeners.add(f), () => void listeners.delete(f));

const MODS = ["cmd", "ctrl", "alt", "shift"];

/** 表記を正規化する（"cmd+shift+z" → "Cmd+Shift+Z"）。読めなければ null。 */
export function normalize(chord: string): string | null {
  const t = chord.trim();
  const plus = t.endsWith("++");
  const parts = (plus ? t.slice(0, -2) : t).split("+").map((p) => p.trim()).filter(Boolean);
  let key = plus ? "Plus" : parts.pop() ?? "";
  const mods = new Set<string>();
  for (const p of parts) {
    const l = p.toLowerCase();
    const m = l === "command" || l === "mod" ? "cmd" : l === "control" ? "ctrl" : l === "option" || l === "opt" ? "alt" : l;
    if (!MODS.includes(m)) return null;
    mods.add(m);
  }
  if (!key) return null;
  key = key.length === 1 ? key.toUpperCase() : key[0].toUpperCase() + key.slice(1);
  const order = ["cmd", "ctrl", "alt", "shift"].filter((m) => mods.has(m)).map((m) => m[0].toUpperCase() + m.slice(1));
  return [...order, key].join("+");
}

export function chordsOf(id: string): string[] {
  return keymap.bindings[id] ?? SPECIFIC[keymap.preset][id] ?? COMMON[id] ?? [];
}

/** 画面に出す表記（⌘⇧Z）。 */
export function display(chord: string): string {
  const sym: Record<string, string> = {
    Cmd: "⌘",
    Ctrl: "⌃",
    Alt: "⌥",
    Shift: "⇧",
    ArrowLeft: "←",
    ArrowRight: "→",
    ArrowUp: "↑",
    ArrowDown: "↓",
    Space: "Space",
    Backspace: "⌫",
    Delete: "⌦",
    OpenBracket: "[",
    CloseBracket: "]",
    Equals: "=",
    Minus: "-",
    Slash: "/",
    Semicolon: ";",
  };
  return chord
    .split("+")
    .map((p) => sym[p] ?? p)
    .join("");
}

const CODE_NAMES: Record<string, string> = {
  Equal: "Equals",
  BracketLeft: "OpenBracket",
  BracketRight: "CloseBracket",
  NumpadAdd: "Plus",
  NumpadSubtract: "Minus",
  NumpadEnter: "Enter",
};

/** 押されたキーを表記にする（修飾キーだけのときは null）。 */
export function chordOfEvent(e: KeyboardEvent): string | null {
  if (["Meta", "Control", "Alt", "Shift"].includes(e.key)) return null;
  const c = e.code;
  const key = c.startsWith("Key") ? c.slice(3) : c.startsWith("Digit") ? c.slice(5) : c.startsWith("Numpad") && /\d$/.test(c) ? c.slice(6) : (CODE_NAMES[c] ?? c);
  const mac = navigator.platform.toLowerCase().includes("mac");
  const parts: string[] = [];
  if (mac ? e.metaKey : e.ctrlKey) parts.push("Cmd");
  if (mac && e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  parts.push(key);
  return parts.join("+");
}

/** 押されたキーに割り当てた操作。修飾キーの多いもの → 一覧の上にあるものが優先。 */
export function actionFor(e: KeyboardEvent): ActionInfo | null {
  const chord = chordOfEvent(e);
  if (!chord) return null;
  return ACTIONS.find((i) => chordsOf(i.id).some((c) => normalize(c) === chord)) ?? null;
}

/** 同じキーに複数の操作が割り当たっているもの（キー → 操作の ID）。 */
export function conflicts(): [string, string[]][] {
  const by = new Map<string, string[]>();
  for (const i of ACTIONS) for (const c of chordsOf(i.id)) {
    const n = normalize(c);
    if (n) by.set(n, [...(by.get(n) ?? []), i.id]);
  }
  return [...by].filter(([, ids]) => ids.length > 1);
}

export async function loadKeymap() {
  try {
    const v = (await api.keymapLoad()) as { preset?: string; bindings?: Record<string, string[]> } | null;
    if (v) keymap = { preset: v.preset === "after_effects" ? "after_effects" : "standard", bindings: v.bindings ?? {} };
  } catch {
    // 読めなければ標準のまま
  }
  listeners.forEach((l) => l());
}

export async function saveKeymap(next: Keymap): Promise<string> {
  keymap = next;
  listeners.forEach((l) => l());
  const path = await api.keymapSave(next);
  return `ショートカットを保存しました（${path}）`;
}
