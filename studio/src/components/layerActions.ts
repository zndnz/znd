// レイヤーへの操作（右クリックのメニュー・インスペクタ・キー操作で共通）。

import { deleteLayer, duplicateLayer, reorder } from "../ops";
import { editComp, editLayer, set } from "../store";

export function layerActions(id: string) {
  return {
    toggleVisible: () => editLayer("表示", id, (l) => void (l.visible = l.visible === false)),
    toggleLock: () => editLayer("ロック", id, (l) => void (l.locked = !l.locked)),
    toggleSolo: () => editLayer("ソロ", id, (l) => void (l.solo = !l.solo)),
    toggle3d: () => editLayer("3D", id, (l) => void (l.three_d = !l.three_d)),
    toggleMotionBlur: () => editLayer("モーションブラー", id, (l) => void (l.motion_blur = !l.motion_blur)),
    duplicate: () => {
      let created: string | null = null;
      editComp("複製", (c, p) => {
        created = duplicateLayer(p, c, id);
        return created !== null;
      });
      if (created) set({ selected: created });
    },
    reorder: (delta: number) => editComp("重ね順", (c) => reorder(c, id, delta)),
    remove: () => {
      editComp("レイヤーを削除", (c) => {
        const l = c.layers.find((l) => l.id === id);
        if (!l || l.locked) return false;
        deleteLayer(c, id);
      });
      set({ selected: null, selectedKeys: null });
    },
  };
}
