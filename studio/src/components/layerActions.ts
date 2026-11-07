// レイヤーへの操作（右クリックのメニュー・インスペクタ・キー操作で共通）。

import { runOp } from "../actions";
import { editLayer } from "../store";

export function layerActions(id: string) {
  return {
    toggleVisible: () => editLayer("表示", id, (l) => void (l.visible = l.visible === false)),
    toggleLock: () => editLayer("ロック", id, (l) => void (l.locked = !l.locked)),
    toggleSolo: () => editLayer("ソロ", id, (l) => void (l.solo = !l.solo)),
    toggleShy: () => editLayer("シャイ", id, (l) => void (l.shy = !l.shy)),
    toggle3d: () => editLayer("3D", id, (l) => void (l.three_d = !l.three_d)),
    toggleMotionBlur: () => editLayer("モーションブラー", id, (l) => void (l.motion_blur = !l.motion_blur)),
    duplicate: () => void runOp({ op: "duplicate", layer: id }),
    /** "front" / "forward" / "backward" / "back"（タイムラインの行ごと動かす）。 */
    reorder: (to: "front" | "forward" | "backward" | "back") => void runOp({ op: "reorder", layer: id, to }),
    remove: () => void runOp({ op: "delete", layer: id }),
    split: () => void runOp({ op: "split", layer: id }),
    hold: (frames: number) => void runOp({ op: "hold", layer: id, frames }),
    newRow: () => void runOp({ op: "new_row", layer: id }),
    precompose: () => void runOp({ op: "precompose", layer: id }),
    outline: () => void runOp({ op: "outline", layer: id }),
    align: (horizontal: boolean, mode: number) => void runOp({ op: "align", layer: id, horizontal, mode }),
    parent: (parent: string | null) => void runOp({ op: "parent", layer: id, parent }),
  };
}
