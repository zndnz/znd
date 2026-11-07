// ビューア。黒い面の中央にコンポを出し、左上に表示の設定、下の中央に浮かぶツールパレットを置く。
//
// ツール: 選択（選択・移動、モーションパスとマスクの頂点の編集）・手のひら・ズーム・回転・ペン・矩形／楕円マスク。
// 座標の変換と当たり判定・編集は Rust（crates/znd-studio/src/viewer.rs）が行い、ここはポインタの位置を
// コンポ座標に直して渡し、返ってきた補助線を SVG で描く。

import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { Overlay, OverlayVertex, ReviewComment, Vec2, ViewerAt, ViewerOutcome } from "../model";
import { frameToTime } from "../model";
import { adopt, currentComp, editComp, get, propOp, reply, run, selectedLayer, set, type Tool, useUi } from "../store";
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
import { shortcut } from "./Timeline";
import { closeDiff } from "./Windows";

const ZOOMS: [number, string][] = [
  [0, "全体表示"],
  [0.25, "25%"],
  [0.5, "50%"],
  [1, "100%"],
  [2, "200%"],
  [4, "400%"],
];
const RESOLUTIONS: [number, string][] = [
  [1, "フル画質"],
  [2, "1/2 画質"],
  [4, "1/4 画質"],
];
const CHANNELS: [number, string][] = [
  [0, "RGB"],
  [1, "R"],
  [2, "G"],
  [3, "B"],
  [4, "輝度"],
];
/** 画面上で掴める距離（px）。 */
const GRAB = 8;
const PATH = "oklch(0.82 0.12 220)";
const MASK = "oklch(0.88 0.16 90)";

/** 表示用の画像。チャンネルを選んでいるときは、その成分をグレースケールで出す。 */
function channelImage(rgba: Uint8ClampedArray, channel: number): Uint8ClampedArray {
  const out = new Uint8ClampedArray(rgba);
  if (channel === 0) return out;
  for (let i = 0; i < out.length; i += 4) {
    const v = channel <= 3 ? rgba[i + channel - 1] : 0.2126 * rgba[i] + 0.7152 * rgba[i + 1] + 0.0722 * rgba[i + 2];
    out[i] = out[i + 1] = out[i + 2] = v;
    out[i + 3] = 255;
  }
  return out;
}

export function Viewer() {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const screenCanvas = useRef<HTMLCanvasElement>(null);
  const diffCanvas = useRef<HTMLCanvasElement>(null);
  const [box, setBox] = useState({ w: 800, h: 450 });
  const [rendering, setRendering] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [screenStats, setScreenStats] = useState<number[] | null>(null);
  const [hover, setHover] = useState<{ p: Vec2; rgba: number[] } | null>(null);
  const [overlay, setOverlay] = useState<Overlay | null>(null);
  const [penHover, setPenHover] = useState<Vec2 | null>(null);
  const [shape, setShape] = useState<[Vec2, Vec2] | null>(null);
  const comp = useUi(currentComp);
  const revision = useUi((s) => s.snap?.revision ?? 0);
  const frame = useUi((s) => s.frame);
  const compId = useUi((s) => s.compId);
  const downsample = useUi((s) => s.downsample);
  const zoom = useUi((s) => s.viewZoom);
  const pan = useUi((s) => s.viewPan);
  const tool = useUi((s) => s.tool);
  const channel = useUi((s) => s.channel);
  const showGrid = useUi((s) => s.showGrid);
  const showGuides = useUi((s) => s.showGuides);
  const screening = useUi((s) => s.screening);
  const selected = useUi((s) => s.selected);
  const activeMask = useUi((s) => s.activeMask);
  const activeVertex = useUi((s) => s.activeVertex);
  const diffLabel = useUi((s) => s.diffLabel);
  const diffGen = useUi((s) => s.diffGen);
  const reviews = useUi((s) => (s.project?.reviews as ReviewComment[] | undefined) ?? []);
  const [penTick, setPenTick] = useState(0);
  // レイヤーを選び直したら、編集中のマスクの頂点を外す
  useEffect(() => set({ activeMask: 0, activeVertex: null }), [selected]);

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
      void api.renderError().then((e) => alive && setError(e));
      const c = canvas.current;
      if (!f || !c) return;
      if (c.width !== f.width || c.height !== f.height) {
        c.width = f.width;
        c.height = f.height;
      }
      c.getContext("2d")?.putImageData(new ImageData(channelImage(f.rgba, channel) as Uint8ClampedArray<ArrayBuffer>, f.width, f.height), 0, 0);
      if (screening) {
        const sc = await api.screening(frame, downsample);
        const o = screenCanvas.current;
        if (!alive || !sc) return;
        setScreenStats(sc.stats);
        if (o && sc.rgba) {
          o.width = sc.width;
          o.height = sc.height;
          o.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(sc.rgba), sc.width, sc.height), 0, 0);
        }
      }
    };
    void draw();
    const un = api.onFrameReady(() => void draw());
    return () => {
      alive = false;
      void un.then((u) => u());
    };
  }, [compId, frame, downsample, revision, channel, screening]);

  // 版との違いの画像
  useEffect(() => {
    if (!diffLabel) return;
    void api.diffImage().then((d) => {
      const c = diffCanvas.current;
      if (!d || !c) return;
      c.width = d.width;
      c.height = d.height;
      c.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(d.rgba), d.width, d.height), 0, 0);
    });
  }, [diffLabel, diffGen]);

  if (!comp) return <div className="viewer" ref={wrap} />;
  const fit = Math.min((box.w - 80) / comp.width, (box.h - 140) / comp.height);
  const scale = zoom > 0 ? zoom : Math.max(0.02, fit);
  const w = comp.width * scale;
  const h = comp.height * scale;
  const left = (box.w - w) / 2 + pan[0];
  const top = (box.h - h) / 2 - 24 + pan[1];

  const at = (): ViewerAt => {
    const s = get();
    return { comp: s.compId, frame: s.frame, layer: s.selected, mask: s.activeMask, grab: GRAB / scale };
  };
  const toComp = (e: { clientX: number; clientY: number }): Vec2 => {
    const r = wrap.current!.getBoundingClientRect();
    return [(e.clientX - r.left - left) / scale, (e.clientY - r.top - top) / scale];
  };
  const refreshOverlay = () => setPenTick((t) => t + 1);
  const absorb = (o: ViewerOutcome) => {
    if (o.snapshot) adopt(o.snapshot);
    if (o.select !== undefined) set({ selected: o.select, selectedFootage: null });
    if (o.mask !== null) set({ activeMask: o.mask });
    if (o.vertex !== undefined) set({ activeVertex: o.vertex });
    if (o.status) set({ status: o.status });
    refreshOverlay();
  };

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button === 2) return;
    const panning = tool === "hand" || e.button === 1 || (e.altKey && tool === "select" && e.button === 0 && !selectedLayer());
    const start = { x: e.clientX, y: e.clientY, pan: get().viewPan };
    const p0 = toComp(e);
    let moved = false;
    let drag: unknown = null;
    let rotateLast: number | null = null;
    let rotateBase = 0;
    let busy = false;
    let pending: { point: Vec2; alt: boolean } | null = null;
    const gesture = get().gesture;

    if (!panning && tool === "zoom") {
      const z = Math.max(0.05, Math.min(16, e.altKey ? scale / 2 : scale * 2));
      // 押した位置が動かないように、表示位置も合わせる
      const r = wrap.current!.getBoundingClientRect();
      const cx = e.clientX - r.left;
      const cy = e.clientY - r.top;
      const nl = cx - p0[0] * z;
      const nt = cy - p0[1] * z;
      set({ viewZoom: z, viewPan: [nl - (box.w - comp.width * z) / 2, nt - ((box.h - comp.height * z) / 2 - 24)] });
      return;
    }
    if (!panning && tool === "rotate") {
      const l = selectedLayer();
      if (!l || l.locked || !overlay?.anchor) return void set({ status: "回転するレイヤーを先に選択してください" });
      const a = overlay.anchor;
      rotateLast = Math.atan2(p0[1] - a[1], p0[0] - a[0]) * (180 / Math.PI);
      rotateBase = overlay.rotation;
    }
    if (!panning && ["select", "pen", "rect", "ellipse"].includes(tool) && e.button === 0) {
      busy = true;
      void api.viewerDown(at(), tool, p0).then((o) => {
        busy = false;
        drag = o.drag;
        absorb(o);
        flush();
      });
    }
    // 届く前の動きは、届いてからまとめて送る
    const flush = () => {
      if (!pending || !drag || busy) return;
      const { point, alt } = pending;
      pending = null;
      busy = true;
      void api.viewerMove(at(), drag, point, alt, gesture).then((o) => {
        busy = false;
        drag = o.drag ?? drag;
        absorb(o);
        flush();
      });
    };

    const move = (ev: PointerEvent) => {
      if (!moved && Math.hypot(ev.clientX - start.x, ev.clientY - start.y) < 3) return;
      moved = true;
      const p = toComp(ev);
      if (panning) return set({ viewPan: [start.pan[0] + ev.clientX - start.x, start.pan[1] + ev.clientY - start.y] });
      if (rotateLast !== null && overlay?.anchor) {
        const a = Math.atan2(p[1] - overlay.anchor[1], p[0] - overlay.anchor[0]) * (180 / Math.PI);
        // 180° をまたいでも連続して回るよう、差を ±180° に収める
        const d = ((a - rotateLast + 540) % 360) - 180;
        rotateLast = a;
        rotateBase += d;
        const l = selectedLayer();
        if (l && d !== 0) void propOp("回転", l.id, "transform.rotation", { op: "set", value: [Math.round(rotateBase * 100) / 100] }, true);
        return;
      }
      if (tool === "rect" || tool === "ellipse") setShape([p0, p]);
      pending = { point: p, alt: ev.altKey };
      flush();
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const p = toComp(ev);
      setShape(null);
      if (tool === "select" && !moved && !panning) {
        void api.viewerClick(at(), p, ev.altKey, ev.detail >= 2).then(absorb);
      }
      if ((tool === "rect" || tool === "ellipse") && moved) {
        const finish = () => (busy ? setTimeout(finish, 16) : drag && void api.viewerUp(at(), tool, drag, p, ev.shiftKey).then(absorb));
        finish();
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const p = toComp(e);
    if (tool === "pen") setPenHover(p);
    const c = canvas.current;
    if (!c || p[0] < 0 || p[1] < 0 || p[0] >= comp.width || p[1] >= comp.height) return setHover(null);
    const x = Math.min(c.width - 1, Math.floor((p[0] / comp.width) * c.width));
    const y = Math.min(c.height - 1, Math.floor((p[1] / comp.height) * c.height));
    const px = c.getContext("2d")?.getImageData(x, y, 1, 1).data;
    if (px) setHover({ p, rgba: [...px] });
  };

  const onWheel = (e: React.WheelEvent) => {
    if (e.ctrlKey || e.metaKey) {
      // ピンチ・⌘ + ホイールで拡大縮小
      const z = Math.max(0.05, Math.min(16, scale * Math.exp(-e.deltaY * 0.01)));
      set({ viewZoom: z });
    } else set((s) => ({ viewPan: [s.viewPan[0] - e.deltaX, s.viewPan[1] - e.deltaY] }));
  };

  const s2 = (p: Vec2) => `${p[0] * scale},${p[1] * scale}`;
  const poly = (pts: Vec2[]) => pts.map(s2).join(" ");

  return (
    <div className={`viewer tool-${tool} ${screening ? "screening" : ""}`} ref={wrap} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerLeave={() => setHover(null)} onWheel={onWheel}>
      <div className="stage" style={{ left, top, width: w, height: h }}>
        <canvas ref={canvas} style={{ width: w, height: h }} />
        {screening && <canvas ref={screenCanvas} className="pointer-events-none absolute inset-0" style={{ width: w, height: h, imageRendering: "pixelated" }} />}
        {diffLabel && <canvas ref={diffCanvas} className="absolute inset-0" style={{ width: w, height: h }} />}
        {showGrid && <Grid w={w} h={h} step={(Math.max(comp.width, comp.height) / 16) * scale} />}
        {showGuides && (
          <svg className="guides" viewBox="0 0 100 100" preserveAspectRatio="none">
            <rect x="5" y="5" width="90" height="90" />
            <rect x="10" y="10" width="80" height="80" />
          </svg>
        )}
        {reviews
          .filter((r) => r.comp === comp.id && !r.resolved && r.point && r.time === frameToTime(comp.frame_rate, frame))
          .map((r, i) => (
            <span key={i} className="pointer-events-none absolute" style={{ left: r.point![0] * scale, top: r.point![1] * scale }}>
              <span className="absolute -top-2.5 -left-2.5 size-5 rounded-full border-2 border-sky-400" />
              <span className="absolute top-[-8px] left-3.5 text-xs whitespace-nowrap text-sky-300">
                {r.author}: {r.text}
              </span>
            </span>
          ))}
        {!diffLabel && (
          <OverlayLayer
            key={`${selected}`}
            at={{ comp: compId, frame, layer: selected, mask: activeMask, grab: GRAB / scale }}
            deps={[revision, penTick]}
            onLoad={setOverlay}
          >
            {(o) => (
              <svg className="pointer-events-none absolute inset-0 overflow-visible" width={w} height={h}>
                {o.frame && <polygon points={poly(o.frame)} fill="none" stroke={PATH} strokeOpacity={0.6} strokeWidth={1} />}
                {o.anchor && (
                  <g stroke={PATH} strokeWidth={1} fill="none">
                    <circle cx={o.anchor[0] * scale} cy={o.anchor[1] * scale} r={4} />
                    <line x1={o.anchor[0] * scale - 7} y1={o.anchor[1] * scale} x2={o.anchor[0] * scale + 7} y2={o.anchor[1] * scale} />
                    <line x1={o.anchor[0] * scale} y1={o.anchor[1] * scale - 7} x2={o.anchor[0] * scale} y2={o.anchor[1] * scale + 7} />
                  </g>
                )}
                {o.motion.length > 1 && <polyline points={poly(o.motion)} fill="none" stroke={PATH} strokeWidth={1.5} />}
                {o.motion_keys.map((k, i) => (
                  <g key={i}>
                    {k.handles?.map((hd, j) => (
                      <g key={j}>
                        <line x1={k.p[0] * scale} y1={k.p[1] * scale} x2={hd[0] * scale} y2={hd[1] * scale} stroke={PATH} strokeOpacity={0.7} />
                        <circle cx={hd[0] * scale} cy={hd[1] * scale} r={3} fill={PATH} />
                      </g>
                    ))}
                    <rect x={k.p[0] * scale - 3.5} y={k.p[1] * scale - 3.5} width={7} height={7} fill={o.motion_now === i ? "white" : PATH} stroke="black" />
                  </g>
                ))}
                {o.masks.map((m, mi) => (
                  <g key={mi} opacity={mi === activeMask ? 1 : 0.5}>
                    <polyline points={poly(m.line)} fill="none" stroke={MASK} strokeWidth={mi === activeMask ? 1.5 : 1} />
                    {m.vertices.map((v, vi) => (
                      <Vertex key={vi} v={v} scale={scale} color={MASK} handles={mi === activeMask} on={mi === activeMask && activeVertex === vi} />
                    ))}
                  </g>
                ))}
                {o.pen.length > 0 && (
                  <g>
                    <polyline points={poly([...o.pen.map((v) => v.p), ...(penHover ? [penHover] : [])])} fill="none" stroke="white" strokeWidth={1.5} />
                    {o.pen.map((v, i) => (
                      <Vertex key={i} v={v} scale={scale} color="white" handles size={i === 0 ? 9 : 6} />
                    ))}
                  </g>
                )}
                {shape && (
                  tool === "ellipse" ? (
                    <ellipse
                      cx={((shape[0][0] + shape[1][0]) / 2) * scale}
                      cy={((shape[0][1] + shape[1][1]) / 2) * scale}
                      rx={(Math.abs(shape[1][0] - shape[0][0]) / 2) * scale}
                      ry={(Math.abs(shape[1][1] - shape[0][1]) / 2) * scale}
                      fill="none"
                      stroke="white"
                      strokeWidth={1.5}
                    />
                  ) : (
                    <rect
                      x={Math.min(shape[0][0], shape[1][0]) * scale}
                      y={Math.min(shape[0][1], shape[1][1]) * scale}
                      width={Math.abs(shape[1][0] - shape[0][0]) * scale}
                      height={Math.abs(shape[1][1] - shape[0][1]) * scale}
                      fill="none"
                      stroke="white"
                      strokeWidth={1.5}
                    />
                  )
                )}
              </svg>
            )}
          </OverlayLayer>
        )}
      </div>

      <div className="float top-left">
        <ViewMenu zoom={zoom} downsample={downsample} />
        {rendering && <span className="px-3 text-xs text-muted-foreground">レンダリング中…</span>}
        {diffLabel && (
          <span className="flex items-center gap-1 px-3 text-xs text-amber-400">
            {diffLabel}
            <IconButton icon="close" title="比較表示を閉じる" size="icon-xs" onClick={closeDiff} />
          </span>
        )}
        {screening && screenStats && (
          <span className="px-3 text-xs text-muted-foreground">
            上映プレビュー: 白飛び {(screenStats[0] * 100).toFixed(1)}% / 黒つぶれ {(screenStats[1] * 100).toFixed(1)}% / バンディングの恐れ {(screenStats[2] * 100).toFixed(1)}%
          </span>
        )}
      </div>
      {error && <div className="float top-left mt-10 max-w-[60%] px-3 text-xs text-destructive">{error}</div>}
      {hover && (
        <div className="float bottom-left text-[11px] text-muted-foreground tabular-nums">
          X {hover.p[0].toFixed(0)} Y {hover.p[1].toFixed(0)}　R {hover.rgba[0]} G {hover.rgba[1]} B {hover.rgba[2]} A {hover.rgba[3]}
        </div>
      )}

      <ToolPalette />
    </div>
  );
}

/** 補助線を Rust から受け取って描く（プロジェクト・時刻・選択・編集中のマスクが変わったら取り直す）。 */
function OverlayLayer(props: { at: ViewerAt; deps: unknown[]; onLoad: (o: Overlay | null) => void; children: (o: Overlay) => React.ReactNode }) {
  const [o, setO] = useState<Overlay | null>(null);
  const { at } = props;
  useEffect(() => {
    if (!at.layer) {
      setO(null);
      props.onLoad(null);
      return;
    }
    let alive = true;
    void api.viewerOverlay(at).then((x) => {
      if (!alive) return;
      setO(x);
      props.onLoad(x);
    });
    return () => {
      alive = false;
    };
  }, [at.comp, at.frame, at.layer, at.mask, ...props.deps]);
  return o ? <>{props.children(o)}</> : null;
}

function Vertex(props: { v: OverlayVertex; scale: number; color: string; handles?: boolean; on?: boolean; size?: number }) {
  const { v, scale, color } = props;
  const size = props.size ?? 6;
  const p = [v.p[0] * scale, v.p[1] * scale];
  return (
    <g>
      {props.handles &&
        [v.tan_in, v.tan_out].map(
          (t, i) =>
            t && (
              <g key={i}>
                <line x1={p[0]} y1={p[1]} x2={t[0] * scale} y2={t[1] * scale} stroke={color} strokeOpacity={0.7} />
                <circle cx={t[0] * scale} cy={t[1] * scale} r={3} fill={color} />
              </g>
            ),
        )}
      <rect x={p[0] - size / 2} y={p[1] - size / 2} width={size} height={size} fill={props.on ? "white" : color} stroke="black" />
    </g>
  );
}

function Grid({ w, h, step }: { w: number; h: number; step: number }) {
  const s = Math.max(8, step);
  const xs: number[] = [];
  const ys: number[] = [];
  for (let x = s; x < w; x += s) xs.push(x);
  for (let y = s; y < h; y += s) ys.push(y);
  return (
    <svg className="pointer-events-none absolute inset-0" width={w} height={h}>
      {xs.map((x) => (
        <line key={`x${x}`} x1={x} y1={0} x2={x} y2={h} stroke="white" strokeOpacity={0.1} />
      ))}
      {ys.map((y) => (
        <line key={`y${y}`} x1={0} y1={y} x2={w} y2={y} stroke="white" strokeOpacity={0.1} />
      ))}
    </svg>
  );
}

/** 表示の設定（拡大率・画質・チャンネル・補助表示・上映プレビュー・GPU・フレームの保存）。 */
function ViewMenu({ zoom, downsample }: { zoom: number; downsample: number }) {
  const channel = useUi((s) => s.channel);
  const showGrid = useUi((s) => s.showGrid);
  const showGuides = useUi((s) => s.showGuides);
  const screening = useUi((s) => s.screening);
  const gpu = useUi((s) => s.snap?.gpu ?? null);
  const gpuOn = useUi((s) => !!s.snap?.gpu_on);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button variant="ghost" size="sm" className="rounded-3xl" />}>
        {zoom > 0 ? `${Math.round(zoom * 100)}%` : "全体表示"}
        <span className="text-muted-foreground">{RESOLUTIONS.find((r) => r[0] === downsample)?.[1]}</span>
        {channel > 0 && <span className="text-amber-400">{CHANNELS[channel][1]}</span>}
        <Icon name="chevron" size={14} className="text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-60">
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
        <DropdownMenuGroup>
          <DropdownMenuLabel>チャンネル</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={String(channel)} onValueChange={(v) => set({ channel: Number(v) })}>
            {CHANNELS.map(([c, label]) => (
              <DropdownMenuRadioItem key={label} value={String(c)}>
                {label}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuCheckboxItem checked={showGuides} onCheckedChange={(v) => set({ showGuides: v })}>
          セーフマージン（90% / 80%）
        </DropdownMenuCheckboxItem>
        <DropdownMenuCheckboxItem checked={showGrid} onCheckedChange={(v) => set({ showGrid: v })}>
          グリッド
        </DropdownMenuCheckboxItem>
        <DropdownMenuCheckboxItem checked={screening} onCheckedChange={(v) => set({ screening: v })}>
          上映プレビュー（白飛び・黒つぶれ・バンディング）
        </DropdownMenuCheckboxItem>
        <DropdownMenuSeparator />
        {gpu ? (
          <DropdownMenuCheckboxItem checked={gpuOn} onCheckedChange={(on) => void api.setGpu(on).then(adopt)}>
            エフェクトを GPU で処理（{gpu}）
          </DropdownMenuCheckboxItem>
        ) : (
          <DropdownMenuLabel className="text-xs">GPU は使えません（エフェクトは CPU で処理）</DropdownMenuLabel>
        )}
        <DropdownMenuItem onClick={() => void reply(() => api.saveFrameImage(get().frame, get().downsample))}>
          フレームを画像として保存…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
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

/** 描きかけのペンのパスを確定する（Enter）・捨てる（Esc）。処理したら true。 */
export async function penKey(key: string): Promise<boolean> {
  if (get().tool !== "pen" || !(key === "Enter" || key === "Escape")) return false;
  if (!(await api.penActive())) return false;
  const s = get();
  const o = await api.penFinish({ comp: s.compId, frame: s.frame, layer: s.selected, mask: s.activeMask, grab: 0 }, false, key === "Escape");
  if (o.snapshot) adopt(o.snapshot);
  if (o.mask !== null) set({ activeMask: o.mask });
  if (o.status) set({ status: o.status });
  set((st) => ({ gesture: st.gesture + 1 }));
  return true;
}

/** 選んでいるマスクの頂点を消す。消そうとしたら true。 */
export function deleteActiveVertex(): boolean {
  const s = get();
  if (s.activeVertex === null || !s.selected) return false;
  void api.maskRemoveVertex({ comp: s.compId, frame: s.frame, layer: s.selected, mask: s.activeMask, grab: 0 }, s.activeVertex).then((o) => {
    if (o.snapshot) adopt(o.snapshot);
    if (o.vertex !== undefined) set({ activeVertex: o.vertex });
    if (o.status) set({ status: o.status });
  });
  return true;
}

const TOOLS: [Tool, string, string, string][] = [
  ["select", "select", "選択ツール: 選択・移動。モーションパスとマスクの頂点を掴んで動かせます", "tool_select"],
  ["hand", "hand", "手のひらツール: ビューアをドラッグして動かします", "tool_hand"],
  ["zoom", "zoom", "ズームツール: クリックで拡大、Option + クリックで縮小", "tool_zoom"],
  ["rotate", "rotate", "回転ツール: 選択中のレイヤーをドラッグして回します", "tool_rotate"],
  ["pen", "pen", "ペンツール: クリックで頂点、ドラッグで曲線。始点をクリックで閉じる。Enter で開いたまま確定、Esc で取り消し", "tool_pen"],
  ["rect", "rectMask", "長方形マスクツール: ドラッグで矩形のマスクを作ります（Shift で正方形）", "tool_rect"],
  ["ellipse", "ellipseMask", "楕円形マスクツール: ドラッグで楕円のマスクを作ります（Shift で正円）", "tool_ellipse"],
];

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
      {TOOLS.map(([t, icon, title, action]) => (
        <IconButton key={t} size="icon" icon={icon} title={`${title} (${shortcut(action)})`} on={tool === t} onClick={() => set({ tool: t })} />
      ))}
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
