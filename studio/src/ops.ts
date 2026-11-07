// プロジェクトを書き換える操作。どれも渡されたオブジェクトをその場で書き換える（呼び出し側が複製してから渡す）。

import type { Composition, Layer, Project, Property, Time } from "./model";

/** レイヤーの中のプロパティ（`value` と `keyframes` を持つもの）を、深さ優先ですべてたどる。 */
export function forEachProperty(node: unknown, f: (p: Property<unknown>) => void) {
  if (!node || typeof node !== "object") return;
  if (Array.isArray(node)) return node.forEach((n) => forEachProperty(n, f));
  const o = node as Record<string, unknown>;
  if ("value" in o && (Array.isArray(o.keyframes) || "expression" in o || "spatial" in o || Object.keys(o).every((k) => ["value", "keyframes", "spatial", "expression"].includes(k)))) {
    f(o as unknown as Property<unknown>);
    return;
  }
  for (const v of Object.values(o)) forEachProperty(v, f);
}

/** レイヤーを時間方向に動かす（イン点・アウト点・ソースの開始・キーフレームをまとめて）。 */
export function shiftLayer(layer: Layer, dt: Time) {
  layer.in_point += dt;
  layer.out_point += dt;
  layer.start_time += dt;
  // キーフレームはレイヤーと一緒に動かす（どのプロパティのものも）
  forEachProperty(layer, (p) => {
    for (const k of p.keyframes ?? []) k.time += dt;
  });
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
