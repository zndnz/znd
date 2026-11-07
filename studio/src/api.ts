// Rust（crates/znd-studio）の呼び出し。ここ以外から invoke しない。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { EffectInfo, EffectInstance, Layer, Project, Snapshot, Vec2 } from "./model";

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
  newDemo: () => invoke<Snapshot>("new_demo"),
  open: (path?: string) => invoke<Snapshot | null>("open", { path: path ?? null }),
  save: (choose: boolean) => invoke<Snapshot | null>("save", { choose }),
  import: (comp: string, frame: number, sequence: boolean) => invoke<Snapshot | null>("import", { comp, frame, sequence }),
  makeLayer: (kind: string, comp: string, frame: number) => invoke<Layer>("make_layer", { kind, comp, frame }),
  evalLayer: (comp: string, layer: string, frame: number) => invoke<Evaluated | null>("eval_layer", { comp, layer, frame }),
  effectCatalog: () => invoke<EffectInfo[]>("effect_catalog"),
  makeEffect: (kind: string, comp: string, layer: string) => invoke<EffectInstance>("make_effect", { kind, comp, layer }),
  cached: (total: number, downsample: number) => invoke<number[]>("cached", { total, downsample }),

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
