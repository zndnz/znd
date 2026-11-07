// プロジェクトを書き換える操作。どれも渡されたオブジェクトをその場で書き換える（呼び出し側が複製してから渡す）。

import type { Composition, Keyframe, Layer, Project, Property, Time } from "./model";

/** トランスフォームのプロパティの名前（タイムラインとインスペクタで同じ順に並べる）。 */
export const TRANSFORM_PROPS = [
  { path: "anchor", label: "アンカーポイント", dims: 2, unit: "" },
  { path: "position", label: "位置", dims: 2, unit: "" },
  { path: "scale", label: "スケール", dims: 2, unit: "%" },
  { path: "rotation", label: "回転", dims: 1, unit: "°" },
  { path: "opacity", label: "不透明度", dims: 1, unit: "%" },
] as const;

export const THREE_D_PROPS = [
  { path: "position_z", label: "位置 Z", dims: 1, unit: "" },
  { path: "rotation_x", label: "X 回転", dims: 1, unit: "°" },
  { path: "rotation_y", label: "Y 回転", dims: 1, unit: "°" },
] as const;

export type PropPath = (typeof TRANSFORM_PROPS)[number]["path"] | (typeof THREE_D_PROPS)[number]["path"];
type Value = number | [number, number];

export function getProp(layer: Layer, path: PropPath): Property<Value> | undefined {
  return (layer.transform as unknown as Record<string, Property<Value> | undefined>)[path];
}

function propOf(layer: Layer, path: PropPath): Property<Value> {
  const tr = layer.transform as unknown as Record<string, Property<Value>>;
  tr[path] ??= { value: 0 };
  return tr[path];
}

export const isAnimated = (p: Property<unknown> | undefined) => !!p && (p.keyframes?.length ?? 0) > 0;

function linearKey<V>(time: Time, value: V): Keyframe<V> {
  return { time, value, interp_in: "linear", interp_out: "linear" };
}

/** 値を書き換える。アニメーション中ならその時刻のキーを（無ければ足して）、そうでなければ静的な値を書き換える。 */
export function setValue(layer: Layer, path: PropPath, t: Time, value: Value) {
  const p = propOf(layer, path);
  if (!isAnimated(p)) {
    p.value = value;
    return;
  }
  const keys = p.keyframes!;
  const i = keys.findIndex((k) => k.time === t);
  if (i >= 0) keys[i].value = value;
  else {
    keys.push(linearKey(t, value));
    keys.sort((a, b) => a.time - b.time);
  }
}

/** アニメーションのオン／オフ。オンにすると今の値でキーを打ち、オフにすると今の値で固定する。 */
export function toggleAnimation(layer: Layer, path: PropPath, t: Time, current: Value) {
  const p = propOf(layer, path);
  if (isAnimated(p)) {
    p.keyframes = [];
    p.value = current;
  } else {
    p.keyframes = [linearKey(t, current)];
  }
}

/** 今の時刻にキーがあれば外し、無ければ今の値で打つ（アニメーション中のプロパティの ◆ ボタン）。 */
export function toggleKeyAt(layer: Layer, path: PropPath, t: Time, current: Value) {
  const p = propOf(layer, path);
  const keys = (p.keyframes ??= []);
  const i = keys.findIndex((k) => k.time === t);
  if (i >= 0) {
    keys.splice(i, 1);
    if (keys.length === 0) p.value = current;
  } else {
    keys.push(linearKey(t, current));
    keys.sort((a, b) => a.time - b.time);
  }
}

/** キーを別の時刻へ動かす。動かした先に別のキーがあれば置き換える。 */
export function moveKey(layer: Layer, path: PropPath, from: Time, to: Time) {
  const keys = propOf(layer, path).keyframes ?? [];
  const k = keys.find((k) => k.time === from);
  if (!k || from === to) return;
  const rest = keys.filter((x) => x !== k && x.time !== to);
  k.time = to;
  propOf(layer, path).keyframes = [...rest, k].sort((a, b) => a.time - b.time);
}

export function deleteKey(layer: Layer, path: PropPath, t: Time) {
  const p = propOf(layer, path);
  const keys = p.keyframes ?? [];
  const k = keys.find((k) => k.time === t);
  if (!k) return;
  p.keyframes = keys.filter((x) => x !== k);
  if (p.keyframes.length === 0) p.value = k.value;
}

/** キーフレームのある時刻（全プロパティ、重複なし）。畳んだレイヤーのバーに点で出す。 */
export function keyTimes(layer: Layer): Time[] {
  const set = new Set<Time>();
  for (const { path } of [...TRANSFORM_PROPS, ...THREE_D_PROPS]) for (const k of getProp(layer, path)?.keyframes ?? []) set.add(k.time);
  return [...set].sort((a, b) => a - b);
}

/** レイヤーを時間方向に動かす（イン点・アウト点・ソースの開始・キーフレームをまとめて）。 */
export function shiftLayer(layer: Layer, dt: Time) {
  layer.in_point += dt;
  layer.out_point += dt;
  layer.start_time += dt;
  for (const { path } of [...TRANSFORM_PROPS, ...THREE_D_PROPS]) {
    for (const k of getProp(layer, path)?.keyframes ?? []) k.time += dt;
  }
}

export function findLayer(project: Project, compId: string, layerId: string): Layer | undefined {
  return findComp(project, compId)?.layers.find((l) => l.id === layerId);
}

export function findComp(project: Project, compId: string): Composition | undefined {
  return project.compositions.find((c) => c.id === compId);
}

/** 使われていない ID（`prefix_N`）。 */
export function freshId(project: Project, prefix: string): string {
  const used = new Set<string>();
  for (const c of project.compositions) {
    used.add(c.id);
    for (const l of c.layers) used.add(l.id);
  }
  for (const f of project.footage) used.add(f.id);
  for (let i = 1; ; i++) if (!used.has(`${prefix}_${i}`)) return `${prefix}_${i}`;
}

/** 重ね順を 1 つ動かす（`delta` が負なら前面へ）。 */
export function reorder(comp: Composition, layerId: string, delta: number) {
  const i = comp.layers.findIndex((l) => l.id === layerId);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= comp.layers.length) return;
  [comp.layers[i], comp.layers[j]] = [comp.layers[j], comp.layers[i]];
}

/** レイヤーを複製して、元のすぐ上に置く。新しい ID を返す。 */
export function duplicateLayer(project: Project, comp: Composition, layerId: string): string | null {
  const i = comp.layers.findIndex((l) => l.id === layerId);
  if (i < 0) return null;
  const copy: Layer = structuredClone(comp.layers[i]);
  copy.id = freshId(project, "layer");
  copy.name = `${copy.name} のコピー`;
  comp.layers.splice(i, 0, copy);
  return copy.id;
}

/** レイヤーを消す。親にしていたレイヤーは親を外し、トラックマットに使っていたものはマットを外す。 */
export function deleteLayer(comp: Composition, layerId: string) {
  comp.layers = comp.layers.filter((l) => l.id !== layerId);
  for (const l of comp.layers) {
    if (l.parent === layerId) l.parent = null;
    const matte = l.track_matte as { layer?: string } | null | undefined;
    if (matte?.layer === layerId) l.track_matte = null;
  }
}
