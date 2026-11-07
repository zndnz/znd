// プロジェクト（左）。コンポと素材の一覧。コンポを選ぶとそのコンポを開き、素材を選ぶとインスペクタに情報が出る。

import { api } from "../api";
import { type FootageItem, FLICKS, seconds } from "../model";
import { freshId } from "../ops";
import { edit, get, run, set, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuShortcut, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
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
  set({ compId: id, selected: null, selectedFootage: null, selectedKeys: null, frame: 0, playing: false, workArea: null, tlZoom: 1, tlStart: 0 });
}

const footageIcon = (f: FootageItem) => (f.width === 0 ? "audio" : f.frame_rate ? "footage" : "picture");

export function ProjectList() {
  const project = useUi((s) => s.project);
  const compId = useUi((s) => s.compId);
  const selFootage = useUi((s) => s.selectedFootage);
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

  const item = (key: string, selected: boolean, icon: string, name: string, meta: string, title: string, onClick: () => void, onDelete: () => void) => (
    <ContextMenu key={key}>
      <ContextMenuTrigger
        render={
          <div
            className={cn("flex h-9 items-center gap-2.5 rounded-3xl px-3 text-sm", selected ? "bg-primary text-primary-foreground" : "hover:bg-muted")}
            title={title}
            onClick={onClick}
          />
        }
      >
        <Icon name={icon} className={selected ? "opacity-80" : "text-muted-foreground"} />
        <span className="flex-1 truncate">{name}</span>
        <span className={cn("text-xs tabular-nums", selected ? "opacity-70" : "text-muted-foreground")}>{meta}</span>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem variant="destructive" onClick={onDelete}>
          削除（使われていないものだけ）
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );

  return (
    <aside className="sidebar">
      <div className="flex h-12 items-center pr-2 pl-4">
        <span className="flex-1 font-semibold">プロジェクト</span>
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" className="text-muted-foreground" aria-label="追加" />}>
            <Icon name="plus" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-60">
            <DropdownMenuItem onClick={newComposition}>
              <Icon name="comp" />
              新規コンポジション
              <DropdownMenuShortcut>⌥⌘N</DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => void run(() => api.import(get().compId, get().frame, false), "素材を読み込みました")}>
              <Icon name="image" />
              素材を読み込む…
              <DropdownMenuShortcut>⌘I</DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => void run(() => api.import(get().compId, get().frame, true), "連番を読み込みました")}>
              <Icon name="footage" />
              連番画像を読み込む…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <div className="flex flex-col gap-0.5 overflow-y-auto px-2 pb-4">
        <div className="px-3 pt-3 pb-1.5 text-xs text-muted-foreground">コンポジション</div>
        {project.compositions.map((c) =>
          item(
            c.id,
            c.id === compId && !selFootage,
            "comp",
            c.name,
            `${c.width}×${c.height}`,
            `${c.width}×${c.height}・${(c.frame_rate.num / c.frame_rate.den).toFixed(2)} fps・${seconds(c.duration).toFixed(2)} 秒`,
            () => (c.id === compId ? set({ selected: null, selectedFootage: null }) : openComp(c.id)),
            () => removeComp(c.id),
          ),
        )}
        {project.footage.length > 0 && <div className="px-3 pt-4 pb-1.5 text-xs text-muted-foreground">素材</div>}
        {project.footage.map((f) => {
          const length = f.duration ?? f.audio?.duration;
          return item(
            f.id,
            f.id === selFootage,
            footageIcon(f),
            f.name,
            f.width ? `${f.width}×${f.height}` : length !== undefined ? `${seconds(length).toFixed(1)} 秒` : "",
            f.path,
            () => set({ selectedFootage: f.id, selected: null }),
            () => removeFootage(f.id),
          );
        })}
      </div>
    </aside>
  );
}
