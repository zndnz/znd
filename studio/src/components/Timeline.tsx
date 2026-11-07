// タイムライン。左の列は置かず、レイヤーは名前の入った角丸のバーとして時間の上に並べる。
// バーの ▸ で開くと、その下にプロパティの行（キーフレームの ◆）が出る。

import { useLayoutEffect, useRef, useState } from "react";
import { type Composition, fps, frameToTime, type Layer, type PropInfo, seconds, timecode, timeToFrame } from "../model";
import { shiftLayer } from "../ops";
import { currentComp, editComp, editLayer, get, isKeySelected, keysOp, selectKey, set, setFrame, totalFrames, useUi } from "../store";
import { useLayerKeys } from "./PropRow";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Slider } from "@/components/ui/slider";
import { IconButton, NumberField } from "./controls";
import { Icon } from "./icons";
import { layerActions } from "./layerActions";
import { PluginLayerMenu } from "./Plugins";

const PAD = 18;
const ROW = 36;
const SUB = 26;
const RULER = 30;
const EDGE = 6;

interface View {
  left: number;
  width: number;
  start: number;
  span: number;
}
const xOf = (v: View, frame: number) => v.left + ((frame - v.start) / v.span) * v.width;
const frameAt = (v: View, x: number) => v.start + ((x - v.left) / v.width) * v.span;

/** 目盛りの間隔（フレーム）。ラベルどうしが 70px 以上離れる、きりのよい秒数。 */
function tickStep(fr: number, pxPerFrame: number): number {
  const choices = [1, 2, 5, 10].map((n) => n).concat([fr / 4, fr / 2, fr, fr * 2, fr * 5, fr * 10, fr * 30, fr * 60]);
  for (const c of choices.sort((a, b) => a - b)) if (c * pxPerFrame >= 70) return Math.max(1, Math.round(c));
  return Math.round(fr * 120);
}

export function Timeline() {
  const comp = useUi(currentComp);
  const frame = useUi((s) => s.frame);
  const zoom = useUi((s) => s.tlZoom);
  const start = useUi((s) => s.tlStart);
  const body = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(800);

  useLayoutEffect(() => {
    const el = body.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  if (!comp) return <div className="timeline" />;
  const total = totalFrames();
  const span = total / zoom;
  const view: View = { left: PAD, width: Math.max(1, width - PAD * 2), start: Math.max(0, Math.min(start, total - span)), span };
  const ppf = view.width / span;

  const onWheel = (e: React.WheelEvent) => {
    const rect = body.current!.getBoundingClientRect();
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      const at = frameAt(view, e.clientX - rect.left);
      const maxZoom = Math.max(1, (total * 60) / view.width);
      const z = Math.max(1, Math.min(maxZoom, zoom * Math.exp(-e.deltaY * 0.01)));
      const newSpan = total / z;
      set({ tlZoom: z, tlStart: Math.max(0, Math.min(total - newSpan, at - ((at - view.start) / span) * newSpan)) });
    } else if (Math.abs(e.deltaX) > Math.abs(e.deltaY) || e.shiftKey) {
      const d = (e.shiftKey ? e.deltaY : e.deltaX) / ppf;
      set({ tlStart: Math.max(0, Math.min(total - span, view.start + d)) });
    }
  };

  // ルーラーを押して動かすと、その時刻へ移動する
  const scrub = (e: React.PointerEvent) => {
    const rect = body.current!.getBoundingClientRect();
    const go = (x: number) => setFrame(Math.round(frameAt(view, x - rect.left)));
    go(e.clientX);
    const move = (ev: PointerEvent) => go(ev.clientX);
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const step = tickStep(fps(comp.frame_rate), ppf);
  const ticks: number[] = [];
  for (let f = Math.floor(view.start / step) * step; f <= view.start + span; f += step) if (f >= 0) ticks.push(f);
  const px = xOf(view, frame);

  return (
    <div className="timeline">
      <Transport comp={comp} total={total} />
      <div className="tl-body" ref={body} onWheel={onWheel}>
        <div className="ruler" style={{ height: RULER }} onPointerDown={scrub}>
          <WorkArea view={view} />
          {ticks.map((f) => (
            <span key={f} className="tick" style={{ left: xOf(view, f) }}>
              {seconds(frameToTime(comp.frame_rate, f)).toFixed(2)}
            </span>
          ))}
          <span className="playhead-cap" style={{ left: px }} />
        </div>
        <div className="tracks">
          <div className="tl-grid">
            {ticks.map((f) => (
              <span key={f} style={{ left: xOf(view, f) }} />
            ))}
          </div>
          {comp.layers.map((layer) => (
            <LayerTrack key={layer.id} comp={comp} layer={layer} view={view} />
          ))}
          {comp.layers.length === 0 && <div className="tl-empty">下のパレットからレイヤーを作るか、素材を読み込んでください</div>}
          <div className="tl-spacer" />
        </div>
        <span className="playhead" style={{ left: px }} />
      </div>
    </div>
  );
}

/** 上の段: 再生・現在時刻・スナップ・拡大率・ワークエリア（開始と終了）。 */
function Transport({ comp, total }: { comp: Composition; total: number }) {
  const playing = useUi((s) => s.playing);
  const frame = useUi((s) => s.frame);
  const zoom = useUi((s) => s.tlZoom);
  const snapping = useUi((s) => s.snapping);
  const work = useUi((s) => s.workArea);
  const fr = comp.frame_rate;
  const sec = (f: number) => seconds(frameToTime(fr, f));
  const toFrame = (s: number) => Math.round(s * fps(fr));
  const [a, b] = work ?? [0, total];
  const maxZoom = Math.max(1, total / 8);
  return (
    <div className="transport">
      <IconButton
        icon={playing ? "pause" : "play"}
        title="再生／停止 (Space)"
        size="icon"
        iconSize={18}
        filled
        className="text-foreground"
        onClick={() => set((s) => ({ playing: !s.playing }))}
      />
      <span className="timecode" title={`${frame} / ${total} フレーム・${fps(fr).toFixed(2)} fps`}>
        {timecode(fr, frame)}
      </span>
      <span className="grow" />
      <IconButton icon="magnet" title="スナップ（再生ヘッド・レイヤーの端・キーフレームに吸着）" on={snapping} onClick={() => set((s) => ({ snapping: !s.snapping }))} />
      <Slider
        className="w-40"
        min={0}
        max={1000}
        value={Math.round((Math.log(zoom) / Math.log(maxZoom || 2)) * 1000) || 0}
        aria-label="タイムラインの拡大率"
        onValueChange={(v) => {
          const z = Math.exp(((Array.isArray(v) ? v[0] : v) / 1000) * Math.log(maxZoom || 2));
          set((s) => ({ tlZoom: z, tlStart: Math.max(0, Math.min(total - total / z, s.frame - total / z / 2)) }));
        }}
      />
      <span className="field-label">開始</span>
      <NumberField
        value={sec(a)}
        digits={2}
        step={1 / fps(fr)}
        min={0}
        className="max-w-20"
        onChange={(v) => set({ workArea: [Math.min(toFrame(v), b - 1), b] })}
      />
      <span className="field-label">終了</span>
      <NumberField
        value={sec(b)}
        digits={2}
        step={1 / fps(fr)}
        min={0}
        className="max-w-20"
        onChange={(v) => set({ workArea: [a, Math.max(a + 1, Math.min(total, toFrame(v)))] })}
      />
    </div>
  );
}

function WorkArea({ view }: { view: View }) {
  const work = useUi((s) => s.workArea);
  if (!work) return null;
  return (
    <>
      <span className="work-out" style={{ left: 0, width: Math.max(0, xOf(view, work[0])) }} />
      <span className="work-out" style={{ left: xOf(view, work[1]), right: 0 }} />
    </>
  );
}

/** スナップ先（再生ヘッド・ほかのレイヤーの端・キーフレーム）。`px` 以内なら吸着したフレームを返す。 */
function snapFrame(comp: Composition, f: number, ppf: number, exclude: string): number {
  if (!get().snapping) return Math.round(f);
  const targets = [get().frame];
  for (const l of comp.layers) {
    if (l.id === exclude) continue;
    targets.push(timeToFrame(comp.frame_rate, l.in_point), timeToFrame(comp.frame_rate, l.out_point));
  }
  let best = Math.round(f);
  let dist = 8 / ppf;
  for (const t of targets) {
    if (Math.abs(t - f) < dist) {
      dist = Math.abs(t - f);
      best = t;
    }
  }
  return best;
}

function LayerTrack({ comp, layer, view }: { comp: Composition; layer: Layer; view: View }) {
  const selected = useUi((s) => s.selected === layer.id);
  const open = useUi((s) => !!s.expanded[layer.id]);
  const fr = comp.frame_rate;
  const inF = timeToFrame(fr, layer.in_point);
  const outF = timeToFrame(fr, layer.out_point);
  const x0 = xOf(view, inF);
  const x1 = xOf(view, outF);
  const ppf = view.width / view.span;
  const audio = layer.kind.type === "audio";
  const cls = ["bar", selected && "selected", layer.visible === false && "is-hidden", audio && "audio", layer.locked && "locked"].filter(Boolean).join(" ");

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    set({ selected: layer.id, selectedFootage: null });
    if (layer.locked) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const local = e.clientX - rect.left;
    const mode = local < EDGE ? "in" : local > rect.width - EDGE ? "out" : "move";
    const startX = e.clientX;
    const move = (ev: PointerEvent) => {
      const df = (ev.clientX - startX) / ppf;
      if (mode === "move") {
        const target = snapFrame(comp, inF + df, ppf, layer.id);
        const dt = frameToTime(fr, target) - frameToTime(fr, inF);
        editLayer("レイヤーを移動", layer.id, (l) => {
          const cur = l.in_point - layer.in_point;
          shiftLayer(l, dt - cur);
        }, true);
      } else if (mode === "in") {
        const f = Math.min(outF - 1, snapFrame(comp, inF + df, ppf, layer.id));
        editLayer("イン点", layer.id, (l) => void (l.in_point = frameToTime(fr, f)), true);
      } else {
        const f = Math.max(inF + 1, snapFrame(comp, outF + df, ppf, layer.id));
        editLayer("アウト点", layer.id, (l) => void (l.out_point = frameToTime(fr, f)), true);
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // 開いたときの行: トランスフォーム（3D レイヤーでなければ 2D のものだけ）と、ほかの動きのあるプロパティすべて
  const all = useLayerKeys(layer);
  const three = !!layer.three_d || ["camera", "light", "model"].includes(layer.kind.type);
  const rows = all.filter((p) =>
    p.path.startsWith("transform.") ? three || !["transform.position_z", "transform.anchor_z", "transform.rotation_x", "transform.rotation_y"].includes(p.path) : p.animated,
  );
  return (
    <div className="track">
      <div className="track-row" style={{ height: ROW }}>
        <ContextMenu>
          <ContextMenuTrigger
            render={<div className={cls} style={{ left: x0, width: Math.max(4, x1 - x0) }} title={layer.name} />}
            onPointerDown={onPointerDown}
            onContextMenu={() => set({ selected: layer.id, selectedFootage: null })}
          >
            <button
              type="button"
              className={`twirl-btn ${open ? "open" : ""}`}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={() => set((s) => ({ expanded: { ...s.expanded, [layer.id]: !s.expanded[layer.id] } }))}
              aria-label={open ? "閉じる" : "プロパティを開く"}
            >
              <Icon name="twirl" size={12} />
            </button>
            <span className="bar-name">{layer.name}</span>
            {layer.locked && <Icon name="lock" size={12} className="bar-flag" />}
            {!open && [...new Set(all.flatMap((p) => p.keys.map((k) => k.time)))].map((t) => (
              <span key={t} className="key-dot" style={{ left: xOf(view, timeToFrame(fr, t)) - x0 }} />
            ))}
          </ContextMenuTrigger>
          <LayerMenu layer={layer} />
        </ContextMenu>
      </div>
      {open && rows.map((p) => <PropTrack key={p.path} comp={comp} layer={layer} prop={p} view={view} x0={x0} />)}
    </div>
  );
}

function PropTrack(props: { comp: Composition; layer: Layer; prop: PropInfo; view: View; x0: number }) {
  const { comp, layer, prop, view } = props;
  const fr = comp.frame_rate;
  const keys = prop.keys;
  const ui = useUi((s) => s);
  const ppf = view.width / view.span;
  const xs = keys.map((k) => xOf(view, timeToFrame(fr, k.time)));
  return (
    <div className="track-row sub" style={{ height: SUB }}>
      <span className="prop-label" style={{ left: Math.max(props.x0 + 26, 4) }}>
        {prop.label}
      </span>
      {keys.slice(0, -1).map((k, i) => (
        <span
          key={k.time}
          className={`key-span ${k.kind === "hold" ? "hold" : ""} ${k.curve ? "curve" : ""}`}
          style={{ left: xs[i], width: xs[i + 1] - xs[i] }}
          title={k.curve ? "イージングカーブあり" : undefined}
        />
      ))}
      {keys.map((k, i) => {
        const addr = { path: prop.path, time: k.time };
        const on = isKeySelected(ui, layer.id, addr);
        return (
          <span
            key={k.time}
            className={`key kind-${k.kind} ${on ? "on" : ""}`}
            style={{ left: xs[i] }}
            title={`${prop.label}: ${timecode(fr, timeToFrame(fr, k.time))}`}
            onPointerDown={(e) => {
              e.stopPropagation();
              const f0 = timeToFrame(fr, k.time);
              if (!on || e.shiftKey) selectKey(layer.id, addr, e.shiftKey);
              setFrame(f0);
              if (layer.locked) return;
              // 選んでいるキーをまとめて動かす。動かした量は、最初からの差で決める
              const startX = e.clientX;
              let moved = 0;
              const move = (ev: PointerEvent) => {
                const f = Math.max(0, snapFrame(comp, f0 + (ev.clientX - startX) / ppf, ppf, ""));
                const dt = frameToTime(fr, f) - k.time;
                if (dt === moved) return;
                const step = dt - moved;
                moved = dt;
                void keysOp("キーフレームを移動", { op: "shift", dt: step }, true);
                setFrame(f);
              };
              const up = () => {
                window.removeEventListener("pointermove", move);
                window.removeEventListener("pointerup", up);
              };
              window.addEventListener("pointermove", move);
              window.addEventListener("pointerup", up);
            }}
          />
        );
      })}
    </div>
  );
}

/** 選んだキーフレームを消す（Delete キー）。消したら true。 */
export function deleteSelectedKey(): boolean {
  if (!get().selectedKeys?.keys.length) return false;
  void keysOp("キーフレームを削除", { op: "delete" });
  return true;
}

function LayerMenu({ layer }: { layer: Layer }) {
  const a = layerActions(layer.id);
  return (
    <ContextMenuContent className="w-60">
      <ContextMenuItem onClick={a.toggleVisible}>{layer.visible === false ? "表示する" : "隠す"}</ContextMenuItem>
      <ContextMenuItem onClick={a.toggleLock}>{layer.locked ? "ロックを外す" : "ロック"}</ContextMenuItem>
      <ContextMenuItem onClick={a.toggleSolo}>{layer.solo ? "ソロを外す" : "ソロ"}</ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem onClick={a.duplicate}>
        複製<ContextMenuShortcut>⌘D</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem onClick={() => a.reorder(-1)}>
        前面へ<ContextMenuShortcut>⌘]</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem onClick={() => a.reorder(1)}>
        背面へ<ContextMenuShortcut>⌘[</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem
        onClick={() =>
          editComp("レイヤーを移動", (c) => {
            const target = c.layers.find((x) => x.id === layer.id);
            if (!target || target.locked) return false;
            shiftLayer(target, frameToTime(c.frame_rate, get().frame) - target.in_point);
          })
        }
      >
        再生ヘッドの位置から始める
      </ContextMenuItem>
      <PluginLayerMenu layer={layer.id} />
      <ContextMenuSeparator />
      <ContextMenuItem variant="destructive" onClick={a.remove}>
        削除<ContextMenuShortcut>⌫</ContextMenuShortcut>
      </ContextMenuItem>
    </ContextMenuContent>
  );
}
