// 画面の状態と、編集を Rust に送る仕組み。
//
// 編集は `edit(label, f)` で行う。プロジェクトを複製して `f` で書き換え、画面にはすぐ反映し（待たない）、
// Rust へ `commit` する。Rust が受け付けなかったら（循環する親子など）最後に受け付けられた状態へ戻す。
// ドラッグ中の連続した編集は `coalesce` で 1 回の取り消しにまとめ、送信が詰まったら途中の状態は飛ばす。

import { useSyncExternalStore } from "react";
import { api } from "./api";
import { type Composition, frameCount, type Layer, type Project, type Snapshot, timeToFrame } from "./model";
import { findComp } from "./ops";

export type Tool = "select" | "hand";

export interface SelectedKey {
  layer: string;
  path: string;
  time: number;
}

export interface UiState {
  snap: Snapshot | null;
  /** 画面に出しているプロジェクト（送信中の編集を含む）。 */
  project: Project | null;
  compId: string;
  selected: string | null;
  selectedFootage: string | null;
  selectedKey: SelectedKey | null;
  frame: number;
  playing: boolean;
  tool: Tool;
  /** 1 = フル、2 = 1/2、4 = 1/4。 */
  downsample: number;
  /** ビューアの拡大率（0 = 全体表示）。 */
  viewZoom: number;
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
  status: string;
}

let state: UiState = {
  snap: null,
  project: null,
  compId: "",
  selected: null,
  selectedFootage: null,
  selectedKey: null,
  frame: 0,
  playing: false,
  tool: "select",
  downsample: 2,
  viewZoom: 0,
  viewPan: [0, 0],
  showProject: true,
  showInspector: true,
  expanded: {},
  tlZoom: 1,
  tlStart: 0,
  workArea: null,
  snapping: true,
  status: "",
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
}
const queue: Pending[] = [];
let sending = false;

async function flush() {
  if (sending) return;
  sending = true;
  while (queue.length) {
    const job = queue.shift()!;
    try {
      const snap = await api.commit(job.project, job.label, job.coalesce);
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
  if (coalesce && last && last.coalesce && last.label === label) last.project = next;
  else queue.push({ project: next, label, coalesce });
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

// ---------- 時刻 ----------

export function setFrame(frame: number) {
  const total = totalFrames();
  set({ frame: Math.max(0, Math.min(total - 1, Math.round(frame))) });
}

export function frameOfTime(t: number): number {
  const c = currentComp();
  return c ? timeToFrame(c.frame_rate, t) : 0;
}
