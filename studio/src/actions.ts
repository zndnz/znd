// ショートカット・メニュー・右クリックから呼ぶ操作。znd-app の操作（keymap の Action）と同じものを、
// 画面の状態の変更か、Rust の `op`（crates/znd-studio/src/ops.rs）で行う。

import { api } from "./api";
import { applyRememberedEase } from "./components/Easing";
import { openComp } from "./components/ProjectList";
import { deleteSelectedKey } from "./components/Timeline";
import { deleteActiveVertex } from "./components/Viewer";
import type { KeyAddr, OpResult } from "./model";
import { timeToFrame } from "./model";
import { adopt, currentComp, get, keysOp, run, selectedLayer, set, setFrame, type Tool, totalFrames } from "./store";

/** Rust の `op` を、いまのコンポと再生ヘッドで実行し、結果（新しい状態・選択・メッセージ）を反映する。 */
export async function runOp(op: Record<string, unknown> & { op: string }): Promise<OpResult | null> {
  const s = get();
  try {
    const r = await api.op({ comp: s.compId, frame: s.frame }, op);
    if (r.snapshot) adopt(r.snapshot);
    if (r.comp) openComp(r.comp);
    if (r.select !== undefined) set({ selected: r.select, selectedFootage: null });
    if (r.keys) {
      const layer = typeof op.layer === "string" ? op.layer : get().selected;
      set({ selectedKeys: r.keys.length && layer ? { layer, keys: r.keys } : null });
    }
    if (r.clip) await navigator.clipboard.writeText(r.clip).catch(() => {});
    if (r.status) set({ status: r.status });
    return r;
  } catch (e) {
    set({ status: String(e) });
    return null;
  }
}

/** 選んでいるレイヤーへの操作（選んでいなければ何もしない）。 */
const onLayer = (op: string, extra: Record<string, unknown> = {}) => {
  const id = get().selected;
  if (id) void runOp({ op, layer: id, ...extra });
};

/** 選んでいるレイヤーのキーフレームの時刻（フレーム）。 */
async function keyFrames(): Promise<number[]> {
  const l = selectedLayer();
  const c = currentComp();
  if (!l || !c) return [];
  const props = await api.layerProps(l, 0);
  return [...new Set(props.flatMap((p) => p.keys.map((k) => timeToFrame(c.frame_rate, k.time))))].sort((a, b) => a - b);
}

export async function selectAllKeys() {
  const l = selectedLayer();
  if (!l) return;
  const props = await api.layerProps(l, 0);
  const keys: KeyAddr[] = props.flatMap((p) => p.keys.map((k) => ({ path: p.path, time: k.time })));
  set({ selectedKeys: keys.length ? { layer: l.id, keys } : null });
}

export function copyKeys(): boolean {
  const sel = get().selectedKeys;
  if (!sel?.keys.length) return false;
  void runOp({ op: "copy_keys", layer: sel.layer, keys: sel.keys });
  return true;
}

/** 最後に触ったプロパティ（キーの貼り付け先）。 */
export let keyTarget: { layer: string; path: string } | null = null;
export const setKeyTarget = (layer: string, path: string) => (keyTarget = { layer, path });

export async function pasteKeys(text?: string) {
  const id = get().selected;
  if (!id) return void set({ status: "キーフレームを貼り付けるレイヤーを選択してください" });
  const t = text ?? (await navigator.clipboard.readText().catch(() => ""));
  if (!t) return;
  void runOp({ op: "paste_keys", layer: id, text: t, target: keyTarget?.layer === id ? keyTarget.path : null });
}

export function addMarker(comment = "") {
  void runOp({ op: "marker", comment });
}

/** ショートカットの操作を実行する。 */
export function runAction(id: string) {
  const s = get();
  const total = totalFrames();
  const layer = selectedLayer();
  const c = currentComp();
  switch (id) {
    case "undo":
      return void run(api.undo);
    case "redo":
      return void run(api.redo);
    case "save":
      return void run(() => api.save(false), "保存しました");
    case "delete":
      if (deleteActiveVertex()) return;
      if (deleteSelectedKey()) return;
      return onLayer("delete");
    case "duplicate":
      return onLayer("duplicate");
    case "split_layer":
      return onLayer("split");
    case "trim_in":
    case "trim_out":
    case "move_in":
    case "move_out":
      return onLayer(id);
    case "hold_1":
    case "hold_2":
    case "hold_3":
    case "hold_4":
      return onLayer("hold", { frames: Number(id.slice(5)) });
    case "play":
      return set({ playing: !s.playing });
    case "next_frame":
      return setFrame(s.frame + 1);
    case "prev_frame":
      return setFrame(s.frame - 1);
    case "next_frame_10":
      return setFrame(s.frame + 10);
    case "prev_frame_10":
      return setFrame(s.frame - 10);
    case "first_frame":
      return setFrame(0);
    case "last_frame":
      return setFrame(total - 1);
    case "prev_key":
    case "next_key":
      return void keyFrames().then((fs) => {
        const f = get().frame;
        const target = id === "prev_key" ? fs.filter((x) => x < f).pop() : fs.find((x) => x > f);
        if (target !== undefined) setFrame(target);
      });
    case "go_in":
      return layer && c && setFrame(timeToFrame(c.frame_rate, layer.in_point));
    case "go_out":
      return layer && c && setFrame(timeToFrame(c.frame_rate, layer.out_point) - 1);
    case "marker":
      // 再生中に連打できる（タップ入力）よう、再生は止めない
      return addMarker();
    case "work_start":
      return set((st) => ({ workArea: [s.frame, Math.max(s.frame + 1, st.workArea?.[1] ?? total)] }));
    case "work_end":
      return set((st) => ({ workArea: [Math.min(st.workArea?.[0] ?? 0, s.frame), s.frame + 1] }));
    case "toggle_snap":
      return set((st) => ({ snapping: !st.snapping, status: st.snapping ? "スナップ: オフ" : "スナップ: オン" }));
    case "tl_zoom_in":
    case "tl_zoom_out": {
      const z = Math.max(1, s.tlZoom * (id === "tl_zoom_in" ? 1.25 : 1 / 1.25));
      const span = total / z;
      return set({ tlZoom: z, tlStart: Math.max(0, Math.min(total - span, s.frame - span / 2)) });
    }
    case "select_all_keys":
      return void selectAllKeys();
    case "easy_ease":
      return void keysOp("イージーイーズ", { op: "kind", kind: "easy_ease" });
    case "ease_in":
      return void keysOp("イーズイン", { op: "kind", kind: "ease_in" });
    case "ease_out":
      return void keysOp("イーズアウト", { op: "kind", kind: "ease_out" });
    case "linear_keys":
      return void keysOp("リニア", { op: "kind", kind: "linear" });
    case "hold_keys":
      return void keysOp("停止", { op: "kind", kind: "hold" });
    case "reverse_keys": {
      const sel = s.selectedKeys;
      if (sel) void runOp({ op: "reverse_keys", layer: sel.layer, keys: sel.keys });
      return;
    }
    case "apply_ease":
      return applyRememberedEase();
    case "tool_select":
    case "tool_hand":
    case "tool_zoom":
    case "tool_rotate":
    case "tool_pen":
      return set({ tool: id.slice(5) as Tool });
    case "tool_rect":
      return set({ tool: "rect" });
    case "tool_ellipse":
      return set({ tool: "ellipse" });
    case "show_shortcuts":
      return set((st) => ({ dialog: st.dialog === "shortcuts" ? null : "shortcuts" }));
  }
}
