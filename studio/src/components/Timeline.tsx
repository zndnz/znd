// タイムライン。左の列は置かず、レイヤーは名前の入った角丸のバーとして時間の上に並べる。
// バーの ▸ で開くと、その下にプロパティの行（キーフレームの ◆）が出る。

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { type Composition, fps, frameToTime, type Layer, seconds, timecode, timeToFrame } from "../model";
import { deleteKey, getProp, isAnimated, keyTimes, moveKey, type PropPath, shiftLayer, THREE_D_PROPS, TRANSFORM_PROPS } from "../ops";
import { currentComp, editComp, editLayer, get, set, setFrame, totalFrames, useUi } from "../store";
import { IconButton, MenuItem, NumberField } from "./controls";
import { Icon } from "./icons";
import { layerActions } from "./layerActions";

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
  const [menu, setMenu] = useState<{ x: number; y: number; layer: string } | null>(null);

  useLayoutEffect(() => {
    const el = body.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [menu]);

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
          <div className="grid">
            {ticks.map((f) => (
              <span key={f} style={{ left: xOf(view, f) }} />
            ))}
          </div>
          {comp.layers.map((layer) => (
            <LayerTrack key={layer.id} comp={comp} layer={layer} view={view} onMenu={(x, y) => setMenu({ x, y, layer: layer.id })} />
          ))}
          {comp.layers.length === 0 && <div className="tl-empty">下のパレットからレイヤーを作るか、素材を読み込んでください</div>}
          <div className="tl-spacer" />
        </div>
        <span className="playhead" style={{ left: px }} />
      </div>
      {menu && <LayerMenu {...menu} onClose={() => setMenu(null)} />}
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
      <IconButton icon={playing ? "pause" : "play"} title="再生／停止 (Space)" className="play" size={18} onClick={() => set((s) => ({ playing: !s.playing }))} />
      <span className="timecode" title={`${frame} / ${total} フレーム・${fps(fr).toFixed(2)} fps`}>
        {timecode(fr, frame)}
      </span>
      <span className="grow" />
      <IconButton icon="magnet" title="スナップ（再生ヘッド・レイヤーの端・キーフレームに吸着）" on={snapping} onClick={() => set((s) => ({ snapping: !s.snapping }))} />
      <input
        className="zoom-slider"
        type="range"
        min={0}
        max={1000}
        value={Math.round((Math.log(zoom) / Math.log(maxZoom || 2)) * 1000) || 0}
        title="タイムラインの拡大率"
        onChange={(e) => {
          const z = Math.exp((Number(e.target.value) / 1000) * Math.log(maxZoom || 2));
          set((s) => ({ tlZoom: z, tlStart: Math.max(0, Math.min(total - total / z, s.frame - total / z / 2)) }));
        }}
      />
      <span className="field-label">開始</span>
      <NumberField
        value={sec(a)}
        digits={2}
        step={1 / fps(fr)}
        min={0}
        width={64}
        onChange={(v) => set({ workArea: [Math.min(toFrame(v), b - 1), b] })}
      />
      <span className="field-label">終了</span>
      <NumberField
        value={sec(b)}
        digits={2}
        step={1 / fps(fr)}
        min={0}
        width={64}
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

function LayerTrack({ comp, layer, view, onMenu }: { comp: Composition; layer: Layer; view: View; onMenu: (x: number, y: number) => void }) {
  const selected = useUi((s) => s.selected === layer.id);
  const open = useUi((s) => !!s.expanded[layer.id]);
  const fr = comp.frame_rate;
  const inF = timeToFrame(fr, layer.in_point);
  const outF = timeToFrame(fr, layer.out_point);
  const x0 = xOf(view, inF);
  const x1 = xOf(view, outF);
  const ppf = view.width / view.span;
  const audio = layer.kind.type === "audio";
  const cls = ["bar", selected && "selected", layer.visible === false && "hidden", audio && "audio", layer.locked && "locked"].filter(Boolean).join(" ");

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

  const props = [...TRANSFORM_PROPS, ...(layer.three_d ? THREE_D_PROPS : [])];
  return (
    <div className="track">
      <div className="track-row" style={{ height: ROW }}>
        <div
          className={cls}
          style={{ left: x0, width: Math.max(4, x1 - x0) }}
          onPointerDown={onPointerDown}
          onContextMenu={(e) => {
            e.preventDefault();
            set({ selected: layer.id });
            onMenu(e.clientX, e.clientY);
          }}
          title={layer.name}
        >
          <button
            type="button"
            className={`twirl-btn ${open ? "open" : ""}`}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={() => set((s) => ({ expanded: { ...s.expanded, [layer.id]: !s.expanded[layer.id] } }))}
            title={open ? "閉じる" : "プロパティを開く"}
          >
            <Icon name="twirl" size={10} />
          </button>
          <span className="bar-name">{layer.name}</span>
          {layer.locked && <Icon name="lock" size={12} className="bar-flag" />}
          {!open &&
            keyTimes(layer).map((t) => <span key={t} className="key-dot" style={{ left: xOf(view, timeToFrame(fr, t)) - x0 }} />)}
        </div>
      </div>
      {open &&
        props.map((p) => <PropTrack key={p.path} comp={comp} layer={layer} path={p.path} label={p.label} view={view} x0={x0} />)}
    </div>
  );
}

function PropTrack(props: { comp: Composition; layer: Layer; path: PropPath; label: string; view: View; x0: number }) {
  const { comp, layer, path, view } = props;
  const fr = comp.frame_rate;
  const prop = getProp(layer, path);
  const keys = prop?.keyframes ?? [];
  const sel = useUi((s) => s.selectedKey);
  const ppf = view.width / view.span;
  const xs = keys.map((k) => xOf(view, timeToFrame(fr, k.time)));
  return (
    <div className="track-row sub" style={{ height: SUB }}>
      <span className="prop-label" style={{ left: Math.max(props.x0 + 22, 4) }}>
        {props.label}
      </span>
      {isAnimated(prop) && xs.length > 1 && <span className="key-span" style={{ left: xs[0], width: xs[xs.length - 1] - xs[0] }} />}
      {keys.map((k, i) => {
        const on = sel?.layer === layer.id && sel.path === path && sel.time === k.time;
        return (
          <span
            key={k.time}
            className={`key ${on ? "on" : ""} ${k.interp_out === "hold" ? "hold" : ""}`}
            style={{ left: xs[i] }}
            title={`${props.label}: ${timecode(fr, timeToFrame(fr, k.time))}`}
            onPointerDown={(e) => {
              e.stopPropagation();
              const f0 = timeToFrame(fr, k.time);
              set({ selected: layer.id, selectedKey: { layer: layer.id, path, time: k.time } });
              setFrame(f0);
              if (layer.locked) return;
              const startX = e.clientX;
              let cur = k.time;
              const move = (ev: PointerEvent) => {
                const f = Math.max(0, snapFrame(comp, f0 + (ev.clientX - startX) / ppf, ppf, ""));
                const t = frameToTime(fr, f);
                if (t === cur) return;
                const from = cur;
                cur = t;
                editLayer("キーフレームを移動", layer.id, (l) => moveKey(l, path, from, t), true);
                set({ selectedKey: { layer: layer.id, path, time: t } });
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
  const k = get().selectedKey;
  if (!k) return false;
  editLayer("キーフレームを削除", k.layer, (l) => deleteKey(l, k.path as PropPath, k.time));
  set({ selectedKey: null });
  return true;
}

function LayerMenu({ x, y, layer, onClose }: { x: number; y: number; layer: string; onClose: () => void }) {
  const comp = currentComp();
  const l = comp?.layers.find((l) => l.id === layer);
  if (!comp || !l) return null;
  const a = layerActions(layer);
  const item = (label: string, f: () => void, hint?: string) => (
    <MenuItem
      label={label}
      hint={hint}
      onClick={() => {
        f();
        onClose();
      }}
    />
  );
  return (
    <div className="popover context" style={{ left: x, top: y }} onPointerDown={(e) => e.stopPropagation()}>
      {item(l.visible === false ? "表示する" : "隠す", a.toggleVisible)}
      {item(l.locked ? "ロックを外す" : "ロック", a.toggleLock)}
      {item(l.solo ? "ソロを外す" : "ソロ", a.toggleSolo)}
      <div className="menu-sep" />
      {item("複製", a.duplicate, "⌘D")}
      {item("前面へ", () => a.reorder(-1), "⌘]")}
      {item("背面へ", () => a.reorder(1), "⌘[")}
      {item("再生ヘッドの位置から始める", () =>
        editComp("レイヤーを移動", (c) => {
          const t = frameToTime(c.frame_rate, get().frame);
          const target = c.layers.find((x) => x.id === layer);
          if (!target || target.locked) return false;
          shiftLayer(target, t - target.in_point);
        }),
      )}
      <div className="menu-sep" />
      {item("削除", a.remove, "⌫")}
    </div>
  );
}
