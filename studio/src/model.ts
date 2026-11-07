// プロジェクトの型（Rust の znd-core を serde で JSON にしたもの）。
// 画面で触る項目だけを型にし、それ以外はそのまま持ち回して Rust へ返す（`[key: string]: unknown`）。

/** 時刻は flicks（1/705,600,000 秒）の整数。 */
export type Time = number;
export const FLICKS = 705_600_000;

export type Vec2 = [number, number];
export type Interp = "linear" | "bezier" | "hold";

export interface FrameRate {
  num: number;
  den: number;
}

export interface Keyframe<V> {
  time: Time;
  value: V;
  interp_in?: Interp;
  interp_out?: Interp;
  [key: string]: unknown;
}

export interface Property<V> {
  value: V;
  keyframes?: Keyframe<V>[];
  spatial?: boolean;
  expression?: unknown;
}

export interface Transform {
  anchor: Property<Vec2>;
  position: Property<Vec2>;
  scale: Property<Vec2>;
  rotation: Property<number>;
  opacity: Property<number>;
  position_z?: Property<number>;
  anchor_z?: Property<number>;
  rotation_x?: Property<number>;
  rotation_y?: Property<number>;
}

export type ParamValue =
  | { t: "scalar"; v: Property<number> }
  | { t: "point"; v: Property<Vec2> }
  | { t: "color"; v: Property<[number, number, number, number]> }
  | { t: "bool"; v: boolean }
  | { t: "choice"; v: number }
  | { t: "layer"; v: string | null }
  | { t: "text"; v: string }
  | { t: "curve"; v: Vec2[] };

export interface EffectInstance {
  kind: string;
  name: string;
  enabled?: boolean;
  params: Record<string, ParamValue>;
}

export type LayerKind =
  | { type: "solid"; width: number; height: number; color: [number, number, number] }
  | { type: "footage"; footage: string }
  | { type: "precomp"; comp: string }
  | { type: "null" }
  | { type: "adjustment" }
  | { type: "shape"; items: unknown[] }
  | { type: "text"; document: TextDocument; [key: string]: unknown }
  | { type: "audio"; footage: string }
  | { type: "camera"; [key: string]: unknown }
  | { type: "light"; light?: string; color: [number, number, number]; [key: string]: unknown }
  | { type: "model"; footage: string };

export interface TextDocument {
  text: string;
  font_family: string;
  size: number;
  fill: [number, number, number, number] | null;
  [key: string]: unknown;
}

export interface Layer {
  id: string;
  name: string;
  kind: LayerKind;
  parent?: string | null;
  in_point: Time;
  out_point: Time;
  start_time: Time;
  transform: Transform;
  blend_mode?: string;
  visible?: boolean;
  solo?: boolean;
  shy?: boolean;
  locked?: boolean;
  guide?: boolean;
  label?: number;
  three_d?: boolean;
  motion_blur?: boolean;
  effects?: EffectInstance[];
  track?: number;
  [key: string]: unknown;
}

export interface Composition {
  id: string;
  name: string;
  width: number;
  height: number;
  pixel_aspect?: number;
  frame_rate: FrameRate;
  duration: Time;
  background?: [number, number, number];
  layers: Layer[];
  motion_blur?: { enabled: boolean; shutter_angle: number; shutter_phase: number; samples: number };
  [key: string]: unknown;
}

export interface FootageItem {
  id: string;
  name: string;
  path: string;
  width: number;
  height: number;
  frame_rate?: FrameRate;
  duration?: Time;
  audio?: { duration: Time; [key: string]: unknown };
  [key: string]: unknown;
}

export interface Project {
  format_version: number;
  name: string;
  footage: FootageItem[];
  compositions: Composition[];
  [key: string]: unknown;
}

/** Rust から受け取る状態。 */
export interface Snapshot {
  project: Project;
  revision: number;
  path: string | null;
  dirty: boolean;
  undo: string | null;
  redo: string | null;
  gpu: string | null;
  gpu_on: boolean;
  /** 復元できる自動保存。 */
  recovery: string | null;
}

export interface ParamInfo {
  name: string;
  label: string;
  kind: "float" | "point" | "color" | "bool" | "choice" | "layer" | "text" | "curve";
  min?: number;
  max?: number;
  choices?: string[];
}

export interface EffectInfo {
  kind: string;
  label: string;
  category: string;
  params: ParamInfo[];
}

// ---------- 時刻 ----------

export const fps = (fr: FrameRate) => fr.num / fr.den;

/** フレーム番号 → そのフレームの開始時刻（Rust の `FrameRate::frame_to_time` と同じ切り捨て）。 */
export function frameToTime(fr: FrameRate, frame: number): Time {
  const n = BigInt(Math.round(frame)) * BigInt(FLICKS) * BigInt(fr.den);
  const d = BigInt(fr.num);
  // BigInt の割り算は 0 方向に丸めるので、負のときは床関数に直す（Rust の div_euclid と同じ）
  const q = n / d;
  return Number(n < 0n && q * d !== n ? q - 1n : q);
}

/** 時刻 → その時刻を含むフレーム番号（切り捨て）。 */
export function timeToFrame(fr: FrameRate, t: Time): number {
  return Math.floor((t * fr.num) / (FLICKS * fr.den));
}

export function frameCount(comp: Composition): number {
  const f = timeToFrame(comp.frame_rate, comp.duration);
  return frameToTime(comp.frame_rate, f) < comp.duration ? f + 1 : f;
}

export const seconds = (t: Time) => t / FLICKS;
export const fromSeconds = (s: number): Time => Math.round(s * FLICKS);

export function timecode(fr: FrameRate, frame: number): string {
  const r = Math.max(1, Math.round(fps(fr)));
  const f = Math.max(0, frame);
  const s = Math.floor(f / r);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}:${pad(f % r)}`;
}

export const FRAME_RATES: { label: string; fr: FrameRate }[] = [
  { label: "23.976", fr: { num: 24000, den: 1001 } },
  { label: "24", fr: { num: 24, den: 1 } },
  { label: "25", fr: { num: 25, den: 1 } },
  { label: "29.97", fr: { num: 30000, den: 1001 } },
  { label: "30", fr: { num: 30, den: 1 } },
  { label: "50", fr: { num: 50, den: 1 } },
  { label: "59.94", fr: { num: 60000, den: 1001 } },
  { label: "60", fr: { num: 60, den: 1 } },
];

export const BLEND_MODES: [string, string][] = [
  ["normal", "通常"],
  ["darken", "比較（暗）"],
  ["multiply", "乗算"],
  ["color_burn", "焼き込みカラー"],
  ["linear_burn", "焼き込みリニア"],
  ["darker_color", "カラー比較（暗）"],
  ["add", "加算"],
  ["lighten", "比較（明）"],
  ["screen", "スクリーン"],
  ["color_dodge", "覆い焼きカラー"],
  ["lighter_color", "カラー比較（明）"],
  ["overlay", "オーバーレイ"],
  ["soft_light", "ソフトライト"],
  ["hard_light", "ハードライト"],
  ["linear_light", "リニアライト"],
  ["vivid_light", "ビビッドライト"],
  ["pin_light", "ピンライト"],
  ["hard_mix", "ハードミックス"],
  ["difference", "差の絶対値"],
  ["exclusion", "除外"],
  ["subtract", "減算"],
  ["divide", "除算"],
  ["hue", "色相"],
  ["saturation", "彩度"],
  ["color", "カラー"],
  ["luminosity", "輝度"],
  ["stencil_alpha", "ステンシルアルファ"],
  ["stencil_luma", "ステンシルルミナンス"],
  ["silhouette_alpha", "シルエットアルファ"],
  ["silhouette_luma", "シルエットルミナンス"],
  ["alpha_add", "アルファ追加"],
];

// ---------- プロパティ・キーフレーム・イージング ----------

export type KeyKind = "linear" | "easy_ease" | "ease_in" | "ease_out" | "auto_bezier" | "hold";

export const KEY_KINDS: [KeyKind, string][] = [
  ["linear", "リニア"],
  ["easy_ease", "イージーイーズ"],
  ["ease_in", "イーズイン"],
  ["ease_out", "イーズアウト"],
  ["auto_bezier", "自動ベジェ"],
  ["hold", "停止"],
];

export type EaseDir = "in" | "out" | "in_out";

export type EaseCurve =
  | { type: "bezier"; x1: number; y1: number; x2: number; y2: number }
  | { type: "elastic"; dir: EaseDir; amplitude: number; frequency: number; decay: number }
  | { type: "bounce"; dir: EaseDir; bounces: number; restitution: number }
  | { type: "steps"; count: number };

export interface EasePreset {
  group: string;
  name: string;
  curve: EaseCurve;
  samples: number[];
}

export interface KeyInfo {
  time: Time;
  kind: KeyKind;
  curve: EaseCurve | null;
}

/** Rust の `znd_core::visit` のパスで指すプロパティ 1 つ（エフェクト・マスク・テキスト・シェイプなども含む）。 */
export interface PropInfo {
  path: string;
  label: string;
  vals: number[];
  is_path: boolean;
  animated: boolean;
  expr: string | null;
  keys: KeyInfo[];
}

export type PropEdit =
  | { op: "set"; value: number[] }
  | { op: "toggle_animation" }
  | { op: "add_key" }
  | { op: "remove_key" }
  | { op: "kind"; kind: KeyKind }
  | { op: "expr"; source: string | null }
  | { op: "move_key"; from: Time; to: Time };

/** キーフレーム 1 つ（プロパティのパス + 時刻）。 */
export interface KeyAddr {
  path: string;
  time: Time;
}

export type KeysOp =
  | { op: "curve"; curve: EaseCurve | null }
  | { op: "kind"; kind: KeyKind }
  | { op: "shift"; dt: Time }
  | { op: "delete" }
  | { op: "ease"; ease_in: [number, number] | null; ease_out: [number, number] | null };

// ---------- プラグイン ----------

export interface PluginReport {
  level: string;
  risk: "safe" | "low" | "medium" | "high" | "critical";
  blocks: boolean;
  summary: string;
  findings: { category: string; score: number; title: string; evidence: string[] }[];
  endpoints: string[];
  hash: string;
  files_scanned: number;
}

export interface PluginCommand {
  key: string;
  plugin: string;
  label: string;
  description: string;
  menu: string[];
  shortcut: string | null;
}

export interface PluginPanelInfo {
  key: string;
  plugin: string;
  title: string;
  width: number;
}

export interface PluginState {
  commands: PluginCommand[];
  panels: PluginPanelInfo[];
  filters: { key: string; label: string; stage: string; enabled: boolean }[];
  packages: {
    id: string;
    name: string;
    version: string;
    vendor: string;
    license: string;
    description: string;
    permissions: string[];
    dir: string;
    commands: number;
    panels: number;
    filters: number;
    expressions: number;
    exporters: number;
    effects: number;
    failed: string | null;
    warnings: string[];
    report: PluginReport;
  }[];
  effects: {
    kind: string;
    label: string;
    category: string;
    api: string;
    vendor: string;
    version: string;
    license: string;
    path: string;
    params: number;
    report: PluginReport | null;
  }[];
  blocked: { path: string; name: string; report: PluginReport }[];
  disabled: string[];
  faults: { kind: string; why: string; quarantined: boolean; disabled: boolean }[];
  missing: [string, string][];
  errors: [string, string][];
  logs: string[];
  ofx_messages: string[];
  native_allowed: boolean;
  plugin_dir: string | null;
  crash: [string, string] | null;
  disclaimer: string;
}

/** スクリプトに渡す画面の状態。 */
export interface PluginCtx {
  comp: string;
  layer: string | null;
  frame: number;
}

/** プラグインの実行結果。`actions` は画面が行う操作。 */
export interface PluginOutcome {
  snapshot: Snapshot | null;
  actions: Record<string, unknown>[];
  status: string | null;
  changed: boolean;
}

/** パネルの部品（スクリプトの `panel(state)` が返すもの）。 */
export interface PanelItem {
  type: string;
  id?: string;
  label?: string;
  text?: string;
  value?: unknown;
  min?: number;
  max?: number;
  step?: number;
  speed?: number;
  options?: string[];
  items?: PanelItem[];
}

// ---------- 書き出し・チェック・履歴など ----------

/** 操作の結果。プロジェクトが変わったときだけ `snapshot` がある。 */
export interface Reply {
  snapshot: Snapshot | null;
  status: string;
}

export interface ExportFormat {
  label: string;
  description: string;
  plugin: boolean;
  alpha: boolean;
  crf: boolean;
  kbps: boolean;
  single_frame: boolean;
  params: { id: string; label: string; kind: string; default: string; min: number | null; max: number | null; choices: [string, string][] }[];
}

/** 書き出しの設定（Rust の `jobs::ExportUi`）。 */
export interface ExportUi {
  format: number;
  crf: number;
  kbps: number;
  audio: boolean;
  pad: number;
  transparent: boolean;
  range: [number, number] | null;
  frame: number;
  values: Record<string, string>;
}

export interface ExportJob {
  comp: string;
  path: string;
  settings: ExportUi;
}

export interface CheckResult {
  lines: [boolean, string][];
  flashes: [number, number, string][];
}

export interface TaskStatus {
  running: [string, number] | null;
  finished: { kind: "export" | "check"; ok: boolean; message: string; check: CheckResult | null } | null;
}

export interface ReviewComment {
  comp: string;
  time: Time;
  author: string;
  text: string;
  point?: [number, number];
  resolved?: boolean;
  created?: string;
}

/** Rust の `op` の結果。 */
export interface OpResult {
  snapshot: Snapshot | null;
  status: string;
  /** 操作のあとに選ぶレイヤー（null で選択を外す。undefined なら変えない）。 */
  select?: string | null;
  keys?: KeyAddr[] | null;
  /** コピーしたキーフレーム（クリップボードに入れる）。 */
  clip?: string | null;
  comp?: string | null;
}

// ---------- ビューアの直接操作 ----------

export interface ViewerAt {
  comp: string;
  frame: number;
  layer: string | null;
  mask: number;
  /** 画面上で掴める距離（コンポ座標）。 */
  grab: number;
}

export interface OverlayVertex {
  p: Vec2;
  tan_in: Vec2 | null;
  tan_out: Vec2 | null;
}

/** 選んでいるレイヤーの補助線（コンポ座標）。 */
export interface Overlay {
  frame: [Vec2, Vec2, Vec2, Vec2] | null;
  anchor: Vec2 | null;
  rotation: number;
  motion: Vec2[];
  motion_keys: { p: Vec2; handles: [Vec2, Vec2] | null }[];
  motion_now: number | null;
  masks: { line: Vec2[]; vertices: OverlayVertex[] }[];
  pen: OverlayVertex[];
}

export interface ViewerOutcome {
  snapshot: Snapshot | null;
  drag: unknown | null;
  select?: string | null;
  mask: number | null;
  vertex?: number | null;
  status: string | null;
}
