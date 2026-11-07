// Rust（crates/znd-studio）の呼び出し。ここ以外から invoke しない。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  CheckResult,
  EaseCurve,
  ExportFormat,
  ExportJob,
  ExportUi,
  Reply,
  TaskStatus,
  EasePreset,
  EffectInfo,
  EffectInstance,
  KeyAddr,
  KeysOp,
  Layer,
  PanelItem,
  PluginCtx,
  PluginOutcome,
  PluginState,
  Project,
  PropEdit,
  PropInfo,
  Snapshot,
  Time,
  Vec2,
} from "./model";

export interface Frame {
  width: number;
  height: number;
  rgba: Uint8ClampedArray;
}

export interface Evaluated {
  anchor: Vec2;
  position: Vec2;
  scale: Vec2;
  rotation: number;
  opacity: number;
  position_z: number;
  anchor_z: number;
  rotation_x: number;
  rotation_y: number;
}

export const api = {
  snapshot: () => invoke<Snapshot>("snapshot"),
  commit: (project: Project, label: string, coalesce: boolean) => invoke<Snapshot>("commit", { project, label, coalesce }),
  undo: () => invoke<Snapshot>("undo"),
  redo: () => invoke<Snapshot>("redo"),
  newProject: () => invoke<Snapshot>("new_project"),
  open: (path?: string) => invoke<Reply | null>("open", { path: path ?? null }),
  save: (choose: boolean) => invoke<Snapshot | null>("save", { choose }),
  import: (comp: string, frame: number, sequence: boolean) => invoke<Snapshot | null>("import", { comp, frame, sequence }),
  makeLayer: (kind: string, comp: string, frame: number) => invoke<Layer>("make_layer", { kind, comp, frame }),
  evalLayer: (comp: string, layer: string, frame: number) => invoke<Evaluated | null>("eval_layer", { comp, layer, frame }),
  effectCatalog: () => invoke<EffectInfo[]>("effect_catalog"),
  makeEffect: (kind: string, comp: string, layer: string) => invoke<EffectInstance>("make_effect", { kind, comp, layer }),
  cached: (total: number, downsample: number) => invoke<number[]>("cached", { total, downsample }),

  // ---- 再生（音は Rust が鳴らす） ----
  /** 再生を始める。`start`〜`end` のフレームを繰り返す。 */
  play: (comp: string, frame: number, start: number, end: number) => invoke<void>("play", { comp, frame, start, end }),
  stop: () => invoke<void>("stop"),
  /** いま鳴っている位置のフレーム（音が無ければ null）。`frame` は経過時間から求めたフレーム。 */
  playClock: (comp: string, frame: number) => invoke<number | null>("play_clock", { comp, frame }),

  // ---- 純粋な計算（レイヤーを渡し、書き換えたものを受け取る） ----
  layerProps: (layer: Layer, time: Time) => invoke<PropInfo[]>("layer_props", { layer, time }),
  propEdit: (layer: Layer, path: string, time: Time, op: PropEdit) => invoke<Layer>("prop_edit", { layer, path, time, op }),
  keysOp: (layer: Layer, keys: KeyAddr[], op: KeysOp) => invoke<{ layer: Layer; keys: KeyAddr[] }>("keys_op", { layer, keys, op }),
  curveOf: (layer: Layer, keys: KeyAddr[]) => invoke<EaseCurve | null>("curve_of", { layer, keys }),
  curvePresets: () => invoke<EasePreset[]>("curve_presets"),
  curveSamples: (curve: EaseCurve, n: number) => invoke<number[]>("curve_samples", { curve, n }),
  parseCurve: (text: string) => invoke<EaseCurve | null>("parse_curve", { text }),
  fonts: () => invoke<[string, string[]][]>("fonts"),

  // ---- ファイル・履歴・版・レビュー ----
  importLyrics: (comp: string) => invoke<Reply | null>("import_lyrics", { comp }),
  importCamera: (comp: string) => invoke<Reply | null>("import_camera", { comp }),
  relinkFootage: () => invoke<Reply | null>("relink_footage"),
  collectFiles: () => invoke<string | null>("collect_files"),
  exportCompPackage: (comp: string) => invoke<string | null>("export_comp_package", { comp }),
  importCompPackage: () => invoke<Reply | null>("import_comp_package"),
  saveFrameImage: (frame: number, downsample: number) => invoke<string | null>("save_frame_image", { frame, downsample }),
  recover: (restore: boolean) => invoke<Reply>("recover", { restore }),
  history: () => invoke<{ undo: string[]; redo: string[] }>("history"),
  historyJump: (index: number | null, forward: number | null) => invoke<Snapshot>("history_jump", { index, forward }),
  versions: () => invoke<{ name: string; saved: number; file: string }[] | null>("versions"),
  versionSave: (name: string) => invoke<string>("version_save", { name }),
  versionRestore: (file: string, name: string) => invoke<Reply>("version_restore", { file, name }),
  versionCompare: (file: string, name: string, comp: string, frame: number) => invoke<string>("version_compare", { file, name, comp, frame }),
  async diffImage(): Promise<{ width: number; height: number; rgba: Uint8ClampedArray } | null> {
    const buf = await invoke<ArrayBuffer>("diff_image");
    if (buf.byteLength < 8) return null;
    const head = new DataView(buf, 0, 8);
    const width = head.getUint32(0, true);
    const height = head.getUint32(4, true);
    return { width, height, rgba: new Uint8ClampedArray(buf, 8, width * height * 4) };
  },
  diffClose: () => invoke<void>("diff_close"),
  reviewAdd: (comp: string, frame: number, text: string, point: [number, number] | null) => invoke<Reply>("review_add", { comp, frame, text, point }),
  creditsText: (comp: string) => invoke<string>("credits_text", { comp }),
  productionLog: () => invoke<string>("production_log"),
  licenses: () => invoke<{ components: { name: string; license: string; usage: string }[]; notice: string }>("licenses"),

  // ---- 書き出し・提出前チェック（バックグラウンド） ----
  exportFormats: () => invoke<ExportFormat[]>("export_formats"),
  exportWarnings: (comp: string) => invoke<string[]>("export_warnings", { comp }),
  exportPick: (comp: string, settings: ExportUi) => invoke<string | null>("export_pick", { comp, settings }),
  exportStart: (jobs: ExportJob[], report: boolean) => invoke<void>("export_start", { jobs, report }),
  taskStatus: () => invoke<TaskStatus>("task_status"),
  taskCancel: () => invoke<void>("task_cancel"),
  checkStart: (comp: string) => invoke<void>("check_start", { comp }),
  checkResult: () => invoke<CheckResult | null>("check_result"),
  exportBreakdown: (comp: string, frame: number) => invoke<string | null>("export_breakdown", { comp, frame }),

  // ---- プラグイン ----
  pluginState: () => invoke<PluginState>("plugin_state"),
  pluginCommand: (key: string, ctx: PluginCtx) => invoke<PluginOutcome>("plugin_command", { key, ctx }),
  pluginPanel: (key: string, ctx: PluginCtx) => invoke<{ items: PanelItem[]; error: string | null }>("plugin_panel", { key, ctx }),
  pluginPanelEvent: (key: string, id: string, value: unknown, ctx: PluginCtx) =>
    invoke<PluginOutcome>("plugin_panel_event", { key, id, value: value ?? null, ctx }),
  pluginLoad: (folder: boolean) => invoke<{ snapshot: Snapshot; status: string } | null>("plugin_load", { folder }),
  pluginRescan: () => invoke<{ snapshot: Snapshot; status: string }>("plugin_rescan"),
  pluginAllow: (path: string) => invoke<{ snapshot: Snapshot; status: string }>("plugin_allow", { path }),
  pluginRevoke: (path: string) => invoke<string>("plugin_revoke", { path }),
  pluginSetNative: (on: boolean) => invoke<string>("plugin_set_native", { on }),
  pluginSetDisabled: (kind: string, off: boolean) => invoke<Snapshot>("plugin_set_disabled", { kind, off }),
  pluginRelease: (kind: string) => invoke<Snapshot>("plugin_release", { kind }),
  pluginCrashAnswer: (disable: boolean) => invoke<Snapshot>("plugin_crash_answer", { disable }),
  pluginSetFilter: (key: string, on: boolean) => invoke<Snapshot>("plugin_set_filter", { key, on }),
  pluginOpenDir: () => invoke<void>("plugin_open_dir"),
  pluginClearLogs: () => invoke<void>("plugin_clear_logs"),

  /** キャッシュにあればそのフレームを返す（無ければ null。できたら onFrameReady が呼ばれる）。 */
  async frame(comp: string, frame: number, downsample: number): Promise<Frame | null> {
    const buf = await invoke<ArrayBuffer>("frame", { comp, frame, downsample });
    if (buf.byteLength < 8) return null;
    const head = new DataView(buf, 0, 8);
    const width = head.getUint32(0, true);
    const height = head.getUint32(4, true);
    return { width, height, rgba: new Uint8ClampedArray(buf, 8, width * height * 4) };
  },

  onFrameReady: (f: () => void): Promise<UnlistenFn> => listen("frame-ready", f),
  onMenu: (f: (id: string) => void): Promise<UnlistenFn> => listen<string>("menu", (e) => f(e.payload)),
};
