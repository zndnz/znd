// プロジェクト（左）。コンポと素材の一覧。コンポを選ぶとそのコンポを開き、素材を選ぶとインスペクタに情報が出る。

import { useEffect, useState } from "react";
import { api } from "../api";
import { type FootageItem, FLICKS, seconds } from "../model";
import { freshId } from "../ops";
import { edit, get, run, set, useUi } from "../store";
import { IconButton, MenuItem, Popover } from "./controls";
import { Icon } from "./icons";

export function newComposition() {
  const cur = get().project?.compositions.find((c) => c.id === get().compId);
  let id = "";
  edit("新規コンポジション", (p) => {
    id = freshId(p, "comp");
    p.compositions.push({
      id,
      name: `コンポ ${p.compositions.length + 1}`,
      width: cur?.width ?? 1920,
      height: cur?.height ?? 1080,
      pixel_aspect: 1,
      frame_rate: cur?.frame_rate ?? { num: 30, den: 1 },
      duration: cur?.duration ?? 10 * FLICKS,
      background: [0, 0, 0],
      layers: [],
    });
  });
  if (id) openComp(id);
}

export function openComp(id: string) {
  set({ compId: id, selected: null, selectedFootage: null, selectedKey: null, frame: 0, playing: false, workArea: null, tlZoom: 1, tlStart: 0 });
}

const footageIcon = (f: FootageItem) => (f.width === 0 ? "audio" : f.frame_rate ? "footage" : "image");

export function ProjectList() {
  const project = useUi((s) => s.project);
  const compId = useUi((s) => s.compId);
  const selFootage = useUi((s) => s.selectedFootage);
  const [menu, setMenu] = useState<{ x: number; y: number; kind: "comp" | "footage"; id: string } | null>(null);
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [menu]);
  if (!project) return <aside className="sidebar" />;

  const used = (fid: string) => project.compositions.some((c) => c.layers.some((l) => (l.kind as { footage?: string }).footage === fid));
  const removeFootage = (fid: string) => {
    if (used(fid)) return set({ status: "この素材を使っているレイヤーがあるため削除できません" });
    edit("素材を削除", (p) => void (p.footage = p.footage.filter((f) => f.id !== fid)));
    set({ selectedFootage: null });
  };
  const removeComp = (cid: string) => {
    if (project.compositions.length <= 1) return set({ status: "最後のコンポジションは削除できません" });
    if (project.compositions.some((c) => c.layers.some((l) => (l.kind as { comp?: string }).comp === cid)))
      return set({ status: "このコンポジションを使っているレイヤーがあるため削除できません" });
    const next = project.compositions.find((c) => c.id !== cid)!.id;
    edit("コンポジションを削除", (p) => void (p.compositions = p.compositions.filter((c) => c.id !== cid)));
    openComp(next);
  };

  return (
    <aside className="sidebar">
      <div className="sidebar-head">
        <span className="sidebar-title">プロジェクト</span>
        <Popover align="end" trigger={(open, toggle) => <IconButton icon="plus" title="追加" on={open} onClick={toggle} />}>
          {(close) => (
            <>
              <MenuItem icon="comp" label="新規コンポジション" hint="⌥⌘N" onClick={() => (close(), newComposition())} />
              <MenuItem
                icon="image"
                label="素材を読み込む…"
                hint="⌘I"
                onClick={() => (close(), void run(() => api.import(get().compId, get().frame, false), "素材を読み込みました"))}
              />
              <MenuItem
                icon="footage"
                label="連番画像を読み込む…"
                onClick={() => (close(), void run(() => api.import(get().compId, get().frame, true), "連番を読み込みました"))}
              />
            </>
          )}
        </Popover>
      </div>
      <div className="list">
        <div className="list-group">コンポジション</div>
        {project.compositions.map((c) => (
          <div
            key={c.id}
            className={`list-item ${c.id === compId && !selFootage ? "selected" : ""}`}
            onClick={() => (c.id === compId ? set({ selected: null, selectedFootage: null }) : openComp(c.id))}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ x: e.clientX, y: e.clientY, kind: "comp", id: c.id });
            }}
            title={`${c.width}×${c.height}・${(c.frame_rate.num / c.frame_rate.den).toFixed(2)} fps・${seconds(c.duration).toFixed(2)} 秒`}
          >
            <Icon name="comp" />
            <span className="list-name">{c.name}</span>
            <span className="list-meta">
              {c.width}×{c.height}
            </span>
          </div>
        ))}
        {project.footage.length > 0 && <div className="list-group">素材</div>}
        {project.footage.map((f) => {
          const length = f.duration ?? f.audio?.duration;
          return (
            <div
              key={f.id}
              className={`list-item ${f.id === selFootage ? "selected" : ""}`}
              onClick={() => set({ selectedFootage: f.id, selected: null })}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, kind: "footage", id: f.id });
              }}
              title={f.path}
            >
              <Icon name={footageIcon(f)} />
              <span className="list-name">{f.name}</span>
              <span className="list-meta">{f.width ? `${f.width}×${f.height}` : length !== undefined ? `${seconds(length).toFixed(1)} 秒` : ""}</span>
            </div>
          );
        })}
      </div>
      {menu && (
        <div className="popover context" style={{ left: menu.x, top: menu.y }} onPointerDown={(e) => e.stopPropagation()}>
          <MenuItem
            label="削除（使われていないものだけ）"
            onClick={() => {
              if (menu.kind === "comp") removeComp(menu.id);
              else removeFootage(menu.id);
              setMenu(null);
            }}
          />
        </div>
      )}
    </aside>
  );
}
