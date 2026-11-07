// 画面の状態と、編集を Rust に送る仕組み。
//
// 編集は `edit(label, f)` で行う。プロジェクトを複製して `f` で書き換え、画面にはすぐ反映し（待たない）、
// Rust へ `commit` する。Rust が受け付けなかったら（循環する親子など）最後に受け付けられた状態へ戻す。
// ドラッグ中の連続した編集は `coalesce` で 1 回の取り消しにまとめ、送信が詰まったら途中の状態は飛ばす。

import { useSyncExternalStore } from "react";
import { api } from "./api";
import {
  type CheckResult,
  type Composition,
  type ExportJob,
  frameCount,
  frameToTime,
  type KeyAddr,
  type Layer,
  type PluginState,
  type Project,
  type PropEdit,
  type Reply,
  type Snapshot,
  type Time,
  timeToFrame,
} from "./model";
import { findComp } from "./ops";

export type Tool = "select" | "hand" | "zoom" | "rotate" | "pen" | "rect" | "ellipse";

/** 選んでいるキーフレーム（1 つのレイヤーの中で複数）。 */
export interface SelectedKeys {
  layer: string;
  keys: KeyAddr[];
}

export interface UiState {
  snap: Snapshot | null;
  /** 画面に出しているプロジェクト（送信中の編集を含む）。 */
  project: Project | null;
  compId: string;
  selected: string | null;
  selectedFootage: string | null;
  selectedKeys: SelectedKeys | null;
  frame: number;
  playing: boolean;
  tool: Tool;
  /** 1 = フル、2 = 1/2、4 = 1/4。 */
  downsample: number;
  /** ビューアの拡大率（0 = 全体表示）。 */
  viewZoom: number;
  /** ビューアに出すチャンネル（0 = RGB、1〜3 = R・G・B、4 = 輝度）。 */
  channel: number;
  showGrid: boolean;
  showGuides: boolean;
  /** 上映プレビュー（白飛び・黒つぶれ・バンディングの恐れを重ねる）。 */
  screening: boolean;
  /** 編集中のマスクと、選んでいる頂点。 */
  activeMask: number;
  activeVertex: number | null;
  /** ドラッグ 1 回ごとに増える番号（同じ番号の間の編集を 1 回の取り消しにまとめる）。 */
  gesture: number;
  viewPan: [number, number];
  showProject: boolean;
  showInspector: boolean;
  /** タイムラインで開いているレイヤー。 */
  expanded: Record<string, boolean>;
  /** 1 = コンポ全体が見える。 */
  tlZoom: number;
  /** 表示範囲の先頭（フレーム）。 */
  tlStart: number;
  workArea: [number, number] | null;
  snapping: boolean;
  /** シャイのレイヤーを隠す。 */
  hideShy: boolean;
  /** タイムラインで、動きのあるプロパティだけを出す。 */
  onlyAnimated: boolean;
  /** タイムラインのレイヤーの絞り込み。 */
  tlSearch: string;
  status: string;
  /** 開いているウィンドウ（書き出し・プラグインの管理など。同時に 1 つ）。 */
  dialog: string | null;
  /** 読み込み済みのプラグイン。 */
  plugins: PluginState | null;
  /** プラグインの定義が変わるたびに増える（エフェクトの一覧などを取り直す）。 */
  pluginGen: number;
  /** 開いているプラグインのパネル。 */
  openPanels: string[];
  /** プラグインからのお知らせ。 */
  alert: string | null;
  /** 実行中のバックグラウンド処理（名前, 進み具合）。 */
  task: [string, number] | null;
  /** 最後の提出前チェックの結果。 */
  check: CheckResult | null;
  /** レンダーキュー。 */
  queue: ExportJob[];
  /** 版と比べた画像を出しているときの説明。 */
  diffLabel: string | null;
  diffGen: number;
  /** グラフエディターで開いているプロパティ。 */
  graphPath: string | null;
  graphOpen: boolean;
  /** エクスプレッションのエラー（編集のたびに調べ直す）。 */
  exprErrors: { layer: string; path: string; line: number | null; message: string }[];
}

let state: UiState = {
  snap: null,
  project: null,
  compId: "",
  selected: null,
  selectedFootage: null,
  selectedKeys: null,
  frame: 0,
  playing: false,
  tool: "select",
  downsample: 2,
  viewZoom: 0,
  channel: 0,
  showGrid: false,
  showGuides: false,
  screening: false,
  activeMask: 0,
  activeVertex: null,
  gesture: 0,
  viewPan: [0, 0],
  showProject: true,
  showInspector: true,
  expanded: {},
  tlZoom: 1,
  tlStart: 0,
  workArea: null,
  snapping: true,
  hideShy: false,
  onlyAnimated: false,
  tlSearch: "",
  status: "",
  dialog: null,
  plugins: null,
  pluginGen: 0,
  openPanels: [],
  alert: null,
  task: null,
  check: null,
  queue: [],
  diffLabel: null,
  diffGen: 0,
  exprErrors: [],
  graphPath: null,
  graphOpen: false,
};

const listeners = new Set<() => void>();

export function get(): UiState {
  return state;
}

export function set(patch: Partial<UiState> | ((s: UiState) => Partial<UiState>)) {
  const p = typeof patch === "function" ? patch(state) : patch;
  state = { ...state, ...p };
  for (const l of listeners) l();
}

export function useUi<T>(select: (s: UiState) => T): T {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => select(state),
  );
}

// ---------- 選択中のもの ----------

export function currentComp(s: UiState = state): Composition | undefined {
  return s.project ? findComp(s.project, s.compId) : undefined;
}

export function selectedLayer(s: UiState = state): Layer | undefined {
  return currentComp(s)?.layers.find((l) => l.id === s.selected);
}

export function totalFrames(s: UiState = state): number {
  const c = currentComp(s);
  return c ? Math.max(1, frameCount(c)) : 1;
}

// ---------- Rust との同期 ----------

/** Rust から受け取った状態を採用する（コンポや選択が消えていたら直す）。 */
export function adopt(snap: Snapshot) {
  set((s) => {
    const project = snap.project;
    const comp = findComp(project, s.compId) ?? project.compositions[0];
    const compId = comp?.id ?? "";
    const selected = comp?.layers.some((l) => l.id === s.selected) ? s.selected : null;
    const selectedFootage = project.footage.some((f) => f.id === s.selectedFootage) ? s.selectedFootage : null;
    return { snap, project, compId, selected, selectedFootage };
  });
}

interface Pending {
  project: Project;
  label: string;
  coalesce: boolean;
  gesture: number;
}
const queue: Pending[] = [];
let sending = false;

async function flush() {
  if (sending) return;
  sending = true;
  while (queue.length) {
    const job = queue.shift()!;
    try {
      const snap = await api.commit(job.project, job.label, job.coalesce, job.gesture);
      // 画面にはもっと新しい編集が出ているかもしれないので、プロジェクトは送信待ちが無いときだけ差し替える
      set(queue.length ? { snap } : { snap, project: state.project });
    } catch (e) {
      queue.length = 0;
      set((s) => ({ project: s.snap?.project ?? s.project, status: `${job.label}できません: ${e}` }));
    }
  }
  sending = false;
}

/** プロジェクトを編集する。`f` には複製を渡すので、その場で書き換えてよい（false を返すと何もしない）。 */
export function edit(label: string, f: (p: Project) => void | boolean, coalesce = false) {
  if (!state.project) return;
  const next: Project = structuredClone(state.project);
  if (f(next) === false) return;
  set({ project: next });
  const last = queue[queue.length - 1];
  const gesture = state.gesture;
  if (coalesce && last && last.coalesce && last.label === label && last.gesture === gesture) last.project = next;
  else queue.push({ project: next, label, coalesce, gesture });
  void flush();
}

/** 現在のコンポを編集する。 */
export function editComp(label: string, f: (c: Composition, p: Project) => void | boolean, coalesce = false) {
  const id = state.compId;
  edit(label, (p) => {
    const c = findComp(p, id);
    return c ? f(c, p) : false;
  }, coalesce);
}

/** レイヤーを編集する（ロック中なら何もしない）。 */
export function editLayer(label: string, layerId: string, f: (l: Layer, c: Composition) => void | boolean, coalesce = false) {
  editComp(label, (c) => {
    const l = c.layers.find((l) => l.id === layerId);
    if (!l || (l.locked && label !== "ロック")) return false;
    return f(l, c);
  }, coalesce);
}

export async function run(f: () => Promise<Snapshot | null>, ok?: string) {
  try {
    const snap = await f();
    if (snap) {
      adopt(snap);
      if (ok) set({ status: ok });
    }
  } catch (e) {
    set({ status: String(e) });
  }
}

/** 操作の結果（新しい状態とメッセージ）を反映する。 */
export async function reply(f: () => Promise<Reply | string | null | void>) {
  try {
    const r = await f();
    if (typeof r === "string") set({ status: r });
    else if (r) {
      if (r.snapshot) adopt(r.snapshot);
      if (r.status) set({ status: r.status });
    }
  } catch (e) {
    set({ status: String(e) });
  }
}

// ---------- 時刻 ----------

export function setFrame(frame: number) {
  const total = totalFrames();
  set({ frame: Math.max(0, Math.min(total - 1, Math.round(frame))) });
}

export function frameOfTime(t: number): number {
  const c = currentComp();
  return c ? timeToFrame(c.frame_rate, t) : 0;
}

/** 再生ヘッドの時刻。 */
export function nowTime(): Time {
  const c = currentComp();
  return c ? frameToTime(c.frame_rate, state.frame) : 0;
}

// ---------- Rust の計算を使う編集 ----------

let chain: Promise<void> = Promise.resolve();

/**
 * レイヤーを Rust の純粋な計算（`f`）で書き換え、編集として送る。前の計算が終わってから次を始めるので、
 * ドラッグ中に続けて呼んでも、いつも最新のレイヤーから計算する。
 */
export function computeLayer(label: string, layerId: string, f: (layer: Layer) => Promise<Layer | null>, coalesce = false): Promise<void> {
  chain = chain.then(async () => {
    const layer = currentComp()?.layers.find((l) => l.id === layerId);
    if (!layer || layer.locked) return;
    try {
      const next = await f(layer);
      if (!next) return;
      editComp(label, (c) => {
        const i = c.layers.findIndex((l) => l.id === layerId);
        if (i < 0) return false;
        c.layers[i] = next;
      }, coalesce);
    } catch (e) {
      set({ status: `${label}できません: ${e}` });
    }
  });
  return chain;
}

/** プロパティ（`path`）への操作を、再生ヘッドの時刻で行う。 */
export function propOp(label: string, layerId: string, path: string, op: PropEdit, coalesce = false) {
  const t = nowTime();
  return computeLayer(label, layerId, (l) => api.propEdit(l, path, t, op), coalesce);
}

/** 選んでいるキーへの一括操作。操作後の選択に置き換える。 */
export function keysOp(label: string, op: Parameters<typeof api.keysOp>[2], coalesce = false) {
  const sel = state.selectedKeys;
  if (!sel || sel.keys.length === 0) return Promise.resolve();
  return computeLayer(label, sel.layer, async (l) => {
    const cur = state.selectedKeys?.layer === sel.layer ? state.selectedKeys.keys : sel.keys;
    const r = await api.keysOp(l, cur, op);
    set({ selectedKeys: r.keys.length ? { layer: sel.layer, keys: r.keys } : null });
    return r.layer;
  }, coalesce);
}

/** キーを選ぶ。`add` なら今の選択に足す（同じレイヤーのとき）／外す。 */
export function selectKey(layer: string, key: KeyAddr, add: boolean) {
  set((s) => {
    const same = (k: KeyAddr) => k.path === key.path && k.time === key.time;
    if (add && s.selectedKeys?.layer === layer) {
      const has = s.selectedKeys.keys.some(same);
      const keys = has ? s.selectedKeys.keys.filter((k) => !same(k)) : [...s.selectedKeys.keys, key];
      return { selected: layer, selectedKeys: keys.length ? { layer, keys } : null };
    }
    return { selected: layer, selectedFootage: null, selectedKeys: { layer, keys: [key] } };
  });
}

export const isKeySelected = (s: UiState, layer: string, key: KeyAddr) =>
  s.selectedKeys?.layer === layer && s.selectedKeys.keys.some((k) => k.path === key.path && k.time === key.time);

// ポインタを離すたびに操作の番号を進める（ドラッグごとに取り消しの単位を分ける）
if (typeof window !== "undefined") window.addEventListener("pointerup", () => set((s) => ({ gesture: s.gesture + 1 })), true);
