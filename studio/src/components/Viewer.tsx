// ビューア。黒い面の中央にコンポを出し、左上に表示の設定、下の中央に浮かぶツールパレットを置く。

import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { currentComp, editComp, get, propOp, run, selectedLayer, set, useUi } from "../store";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Separator } from "@/components/ui/separator";
import { IconButton } from "./controls";
import { Icon } from "./icons";

const ZOOMS: [number, string][] = [
  [0, "全体表示"],
  [0.25, "25%"],
  [0.5, "50%"],
  [1, "100%"],
  [2, "200%"],
];
const RESOLUTIONS: [number, string][] = [
  [1, "フル画質"],
  [2, "1/2 画質"],
  [4, "1/4 画質"],
];

export function Viewer() {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [box, setBox] = useState({ w: 800, h: 450 });
  const [rendering, setRendering] = useState(false);
  const [guides, setGuides] = useState(false);
  const comp = useUi(currentComp);
  const revision = useUi((s) => s.snap?.revision ?? 0);
  const frame = useUi((s) => s.frame);
  const compId = useUi((s) => s.compId);
  const downsample = useUi((s) => s.downsample);
  const zoom = useUi((s) => s.viewZoom);
  const pan = useUi((s) => s.viewPan);
  const tool = useUi((s) => s.tool);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 表示したいフレームを Rust に頼み、できていれば描く。できたら "frame-ready" が来るので頼み直す
  useEffect(() => {
    let alive = true;
    const draw = async () => {
      const f = await api.frame(compId, frame, downsample);
      if (!alive) return;
      setRendering(!f);
      const c = canvas.current;
      if (!f || !c) return;
      if (c.width !== f.width || c.height !== f.height) {
        c.width = f.width;
        c.height = f.height;
      }
      c.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(f.rgba), f.width, f.height), 0, 0);
    };
    void draw();
    const un = api.onFrameReady(() => void draw());
    return () => {
      alive = false;
      void un.then((u) => u());
    };
  }, [compId, frame, downsample, revision]);

  if (!comp) return <div className="viewer" ref={wrap} />;
  const fit = Math.min((box.w - 80) / comp.width, (box.h - 140) / comp.height);
  const scale = zoom > 0 ? zoom : Math.max(0.02, fit);
  const w = comp.width * scale;
  const h = comp.height * scale;
  const left = (box.w - w) / 2 + pan[0];
  const top = (box.h - h) / 2 - 24 + pan[1];

  // 選択ツールでドラッグ: 選んだレイヤーを動かす。手のひらツール（または中ボタン・Alt）でドラッグ: 画面を動かす
  const onPointerDown = (e: React.PointerEvent) => {
    const panning = tool === "hand" || e.button === 1 || e.altKey;
    const layer = selectedLayer();
    if (!panning && (!layer || layer.locked || e.button !== 0)) return;
    const start = { x: e.clientX, y: e.clientY, pan: get().viewPan };
    // 動かし始めの位置（キーフレームを補間した値）。届くまでの動きは届いてから反映する
    let base: [number, number] | null = null;
    let last: [number, number] = [0, 0];
    const apply = () => {
      if (!layer || !base) return;
      const p: [number, number] = [Math.round((base[0] + last[0] / scale) * 10) / 10, Math.round((base[1] + last[1] / scale) * 10) / 10];
      void propOp("移動", layer.id, "transform.position", { op: "set", value: p }, true);
    };
    if (!panning && layer) {
      void api.evalLayer(comp.id, layer.id, get().frame).then((v) => {
        base = v?.position ?? (layer.transform.position.value as [number, number]);
        apply();
      });
    }
    const move = (ev: PointerEvent) => {
      last = [ev.clientX - start.x, ev.clientY - start.y];
      if (panning) set({ viewPan: [start.pan[0] + last[0], start.pan[1] + last[1]] });
      else apply();
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const onWheel = (e: React.WheelEvent) => {
    if (e.ctrlKey || e.metaKey) {
      // ピンチ・⌘ + ホイールで拡大縮小
      const z = Math.max(0.05, Math.min(8, scale * Math.exp(-e.deltaY * 0.01)));
      set({ viewZoom: z });
    } else set((s) => ({ viewPan: [s.viewPan[0] - e.deltaX, s.viewPan[1] - e.deltaY] }));
  };

  return (
    <div className={`viewer tool-${tool}`} ref={wrap} onPointerDown={onPointerDown} onWheel={onWheel}>
      <div className="stage" style={{ left, top, width: w, height: h }}>
        <canvas ref={canvas} style={{ width: w, height: h }} />
        {guides && (
          <svg className="guides" viewBox="0 0 100 100" preserveAspectRatio="none">
            <rect x="5" y="5" width="90" height="90" />
            <rect x="10" y="10" width="80" height="80" />
          </svg>
        )}
      </div>

      <div className="float top-left">
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="sm" className="rounded-3xl" />}>
            {zoom > 0 ? `${Math.round(zoom * 100)}%` : "全体表示"}
            <span className="text-muted-foreground">{RESOLUTIONS.find((r) => r[0] === downsample)?.[1]}</span>
            <Icon name="chevron" size={14} className="text-muted-foreground" />
          </DropdownMenuTrigger>
          <DropdownMenuContent className="w-52">
            <DropdownMenuGroup>
              <DropdownMenuLabel>拡大率</DropdownMenuLabel>
              <DropdownMenuRadioGroup value={String(zoom)} onValueChange={(v) => set({ viewZoom: Number(v), viewPan: [0, 0] })}>
                {ZOOMS.map(([z, label]) => (
                  <DropdownMenuRadioItem key={label} value={String(z)}>
                    {label}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuGroup>
              <DropdownMenuLabel>画質</DropdownMenuLabel>
              <DropdownMenuRadioGroup value={String(downsample)} onValueChange={(v) => set({ downsample: Number(v) })}>
                {RESOLUTIONS.map(([d, label]) => (
                  <DropdownMenuRadioItem key={label} value={String(d)}>
                    {label}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuCheckboxItem checked={guides} onCheckedChange={setGuides}>
              セーフマージン
            </DropdownMenuCheckboxItem>
          </DropdownMenuContent>
        </DropdownMenu>
        {rendering && <span className="px-3 text-xs text-muted-foreground">レンダリング中…</span>}
      </div>

      <ToolPalette />
    </div>
  );
}

const SHAPES: [string, string, string][] = [
  ["rect", "rect", "矩形"],
  ["ellipse", "ellipse", "楕円"],
  ["star", "star", "スター"],
];
const MORE: [string, string, string][] = [
  ["solid", "solid", "平面"],
  ["null", "null", "ヌル"],
  ["adjustment", "adjust", "調整レイヤー"],
  ["camera", "camera", "カメラ"],
  ["light_point", "light", "ライト（ポイント）"],
  ["light_spot", "light", "ライト（スポット）"],
  ["light_parallel", "light", "ライト（平行）"],
  ["light_ambient", "light", "ライト（アンビエント）"],
];

/** 新しいレイヤーを再生ヘッドの位置に作って選ぶ。 */
export async function createLayer(kind: string) {
  const s = get();
  try {
    const layer = await api.makeLayer(kind, s.compId, s.frame);
    editComp("レイヤー追加", (c) => void c.layers.unshift(layer));
    set({ selected: layer.id, selectedFootage: null });
  } catch (e) {
    set({ status: String(e) });
  }
}

function ToolPalette() {
  const tool = useUi((s) => s.tool);
  const [shape, setShape] = useState(0);
  const menu = (items: [string, string, string][], pick?: (i: number) => void) => (
    <DropdownMenuContent side="top" align="center" sideOffset={12} className="w-56">
      {items.map(([kind, icon, label], i) => (
        <DropdownMenuItem key={kind} onClick={() => (pick?.(i), void createLayer(kind))}>
          <Icon name={icon} />
          {label}
        </DropdownMenuItem>
      ))}
    </DropdownMenuContent>
  );
  return (
    <div className="float bottom-center palette">
      <IconButton size="icon" icon="select" title="選択ツール (V)" on={tool === "select"} onClick={() => set({ tool: "select" })} />
      <IconButton size="icon" icon="hand" title="手のひらツール (H)" on={tool === "hand"} onClick={() => set({ tool: "hand" })} />
      <Separator orientation="vertical" className="mx-1.5 my-2" />
      <IconButton size="icon" icon="text" title="テキスト (T)" onClick={() => void createLayer("text")} />
      <IconButton size="icon" icon={SHAPES[shape][1]} title={`シェイプ（${SHAPES[shape][2]}）`} onClick={() => void createLayer(SHAPES[shape][0])} />
      <DropdownMenu>
        <DropdownMenuTrigger render={<Button variant="ghost" size="icon-xs" className="-ml-1 text-muted-foreground" aria-label="シェイプの種類" />}>
          <Icon name="chevron" size={12} />
        </DropdownMenuTrigger>
        {menu(SHAPES, setShape)}
      </DropdownMenu>
      <IconButton
        size="icon"
        icon="image"
        title="素材を読み込む (⌘I)"
        onClick={() => {
          const s = get();
          void run(() => api.import(s.compId, s.frame, false), "素材を読み込みました");
        }}
      />
      <DropdownMenu>
        <DropdownMenuTrigger render={<Button variant="ghost" size="icon" className="text-muted-foreground" aria-label="ほかのレイヤー" />}>
          <Icon name="more" />
        </DropdownMenuTrigger>
        {menu(MORE)}
      </DropdownMenu>
    </div>
  );
}
