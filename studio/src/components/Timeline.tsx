// タイムライン。左の列は置かず、レイヤーは名前の入った角丸のバーとして時間の上に並べる。
// バーの ▸ で開くと、その下にプロパティの行（キーフレームの ◆）が出る。

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../api";
import { type Composition, fps, frameToTime, type Layer, type PropInfo, seconds, timecode, timeToFrame } from "../model";
import { shiftLayer } from "../ops";
import { currentComp, editComp, editLayer, get, isKeySelected, keysOp, selectKey, set, setFrame, totalFrames, useUi } from "../store";
import { useLayerKeys } from "./PropRow";
import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuGroup,
  ContextMenuLabel,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Button } from "@/components/ui/button";
import { addMarker, copyKeys, pasteKeys, runAction, runOp, setKeyTarget } from "../actions";
import { chordsOf, display } from "../keymap";
import { KEY_KINDS } from "../model";
import { Slider } from "@/components/ui/slider";
import { IconButton, NumberField } from "./controls";
import { Icon } from "./icons";
import { layerActions } from "./layerActions";
import { PluginLayerMenu } from "./Plugins";
import { useFlashes } from "./Windows";

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
  const hideShy = useUi((s) => s.hideShy);
  const search = useUi((s) => s.tlSearch);

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
          <Flashes view={view} />
          <BeatGrid comp={comp} view={view} />
          {ticks.map((f) => (
            <span key={f} className="tick" style={{ left: xOf(view, f) }}>
              {seconds(frameToTime(comp.frame_rate, f)).toFixed(2)}
            </span>
          ))}
          <span className="playhead-cap" style={{ left: px }} />
        </div>
        <MarkerLane comp={comp} view={view} />
        <div className="tracks">
          <div className="tl-grid">
            {ticks.map((f) => (
              <span key={f} style={{ left: xOf(view, f) }} />
            ))}
          </div>
          {trackRows(comp.layers)
            .map((row) => row.filter((l) => (!hideShy || !l.shy) && (!search || l.name.toLowerCase().includes(search.toLowerCase()))))
            .filter((row) => row.length > 0)
            .map((row) => (
              <RowTrack key={row[0].id} comp={comp} members={row} view={view} />
            ))}
          {comp.layers.length === 0 && <div className="tl-empty">下のパレットからレイヤーを作るか、素材を読み込んでください</div>}
          <AudioLane comp={comp} view={view} />
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
      <LevelMeter comp={comp} />
      <TimelineFilters />
      <IconButton icon="marker" title={`マーカーを打つ (${shortcut("marker")})`} onClick={() => addMarker()} />
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

/** 提出前チェックで見つかった明滅の区間（赤）。 */
function Flashes({ view }: { view: View }) {
  const flashes = useFlashes();
  return (
    <>
      {flashes.map(([a, b, why], i) => (
        <span
          key={i}
          className="absolute bottom-0 h-1 rounded-full bg-red-500"
          style={{ left: xOf(view, a), width: Math.max(2, xOf(view, b + 1) - xOf(view, a)) }}
          title={why}
        />
      ))}
    </>
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

/** タイムラインの 1 行。重ね順で隣り合い `track` が同じ（0 以外の）レイヤーは、1 行に時間をずらして並ぶ。 */
function trackRows(layers: Layer[]): Layer[][] {
  const rows: Layer[][] = [];
  for (const l of layers) {
    const last = rows[rows.length - 1];
    if (last && l.track && last[0].track === l.track) last.push(l);
    else rows.push([l]);
  }
  return rows;
}

function RowTrack({ comp, members, view }: { comp: Composition; members: Layer[]; view: View }) {
  const selected = useUi((s) => s.selected);
  const active = members.find((l) => l.id === selected) ?? members[0];
  const open = useUi((s) => !!s.expanded[active.id]);
  return (
    <div className="track">
      <div className="track-row" style={{ height: ROW }}>
        {members.map((l) => (
          <Bar key={l.id} comp={comp} layer={l} view={view} />
        ))}
      </div>
      {open && <PropRows comp={comp} layer={active} view={view} />}
    </div>
  );
}

function PropRows({ comp, layer, view }: { comp: Composition; layer: Layer; view: View }) {
  const onlyAnimated = useUi((s) => s.onlyAnimated);
  const all = useLayerKeys(layer);
  const three = !!layer.three_d || ["camera", "light", "model"].includes(layer.kind.type);
  // トランスフォーム（3D レイヤーでなければ 2D のものだけ）と、ほかの動きのあるプロパティすべて
  const rows = all.filter((p) =>
    onlyAnimated ? p.animated : p.path.startsWith("transform.") ? three || !["transform.position_z", "transform.anchor_z", "transform.rotation_x", "transform.rotation_y"].includes(p.path) : p.animated,
  );
  const x0 = xOf(view, timeToFrame(comp.frame_rate, layer.in_point));
  return (
    <>
      {rows.map((p) => (
        <PropTrack key={p.path} comp={comp} layer={layer} prop={p} view={view} x0={x0} />
      ))}
    </>
  );
}

function Bar({ comp, layer, view }: { comp: Composition; layer: Layer; view: View }) {
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

  const all = useLayerKeys(layer);
  const hold = (layer.hold_frames as number | undefined) ?? 1;
  return (
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
            {hold > 1 && <span className="bar-flag text-[10px]">{hold}コマ</span>}
            {layer.shy && <span className="bar-flag text-[10px]">シャイ</span>}
            {layer.locked && <Icon name="lock" size={12} className="bar-flag" />}
            {!open && [...new Set(all.flatMap((p) => p.keys.map((k) => k.time)))].map((t) => (
              <span key={t} className="key-dot" style={{ left: xOf(view, timeToFrame(fr, t)) - x0 }} />
            ))}
          </ContextMenuTrigger>
          <LayerMenu layer={layer} />
        </ContextMenu>
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
          <ContextMenu key={k.time}>
          <ContextMenuTrigger
            render={<span />}
            onContextMenu={() => {
              if (!on) selectKey(layer.id, addr, false);
            }}
            className={`key kind-${k.kind} ${on ? "on" : ""}`}
            style={{ left: xs[i] }}
            title={`${prop.label}: ${timecode(fr, timeToFrame(fr, k.time))}`}
            onPointerDown={(e) => {
              if (e.button !== 0) return;
              e.stopPropagation();
              setKeyTarget(layer.id, prop.path);
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
          <KeyMenu />
          </ContextMenu>
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
  const inRow = useUi((s) => {
    const c = currentComp(s);
    return !!layer.track && (c?.layers.filter((l) => l.track === layer.track).length ?? 0) > 1;
  });
  const hold = (layer.hold_frames as number | undefined) ?? 1;
  return (
    <ContextMenuContent className="w-64">
      <ContextMenuGroup>
        <ContextMenuLabel className="truncate text-xs text-muted-foreground">{layer.name}</ContextMenuLabel>
      </ContextMenuGroup>
      <ContextMenuItem onClick={a.duplicate}>
        複製<ContextMenuShortcut>{shortcut("duplicate")}</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem disabled={!!layer.locked} onClick={a.split}>
        再生ヘッドで分割<ContextMenuShortcut>{shortcut("split_layer")}</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem disabled={!!layer.locked} onClick={() => void runOp({ op: "move_in", layer: layer.id })}>
        再生ヘッドの位置から始める<ContextMenuShortcut>{shortcut("move_in")}</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem onClick={() => a.reorder("front")}>最前面へ</ContextMenuItem>
      <ContextMenuItem onClick={() => a.reorder("forward")}>
        ひとつ前面へ<ContextMenuShortcut>⌘]</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem onClick={() => a.reorder("backward")}>
        ひとつ背面へ<ContextMenuShortcut>⌘[</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem onClick={() => a.reorder("back")}>最背面へ</ContextMenuItem>
      {inRow && <ContextMenuItem onClick={a.newRow}>この行から出す</ContextMenuItem>}
      <ContextMenuSub>
        <ContextMenuSubTrigger>コマ打ち</ContextMenuSubTrigger>
        <ContextMenuSubContent>
          {[1, 2, 3, 4].map((n) => (
            <ContextMenuCheckboxItem key={n} checked={hold === n} onClick={() => a.hold(n)}>
              {n === 1 ? "なし（1 コマ）" : `${n} コマ打ち`}
              <ContextMenuShortcut>{shortcut(`hold_${n}`)}</ContextMenuShortcut>
            </ContextMenuCheckboxItem>
          ))}
        </ContextMenuSubContent>
      </ContextMenuSub>
      <ContextMenuSeparator />
      <ContextMenuItem onClick={a.toggleVisible}>{layer.visible === false ? "表示する" : "隠す"}</ContextMenuItem>
      <ContextMenuItem onClick={a.toggleLock}>{layer.locked ? "ロックを外す" : "ロック"}</ContextMenuItem>
      <ContextMenuItem onClick={a.toggleSolo}>{layer.solo ? "ソロを外す" : "ソロ"}</ContextMenuItem>
      <ContextMenuItem onClick={a.toggleShy}>{layer.shy ? "シャイを外す" : "シャイ"}</ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem onClick={a.precompose}>プリコンポーズ</ContextMenuItem>
      {layer.kind.type === "text" && <ContextMenuItem onClick={a.outline}>テキストをシェイプに変換</ContextMenuItem>}
      <PluginLayerMenu layer={layer.id} />
      <ContextMenuSeparator />
      <ContextMenuItem variant="destructive" disabled={!!layer.locked} onClick={a.remove}>
        削除<ContextMenuShortcut>{shortcut("delete")}</ContextMenuShortcut>
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

/** 選んでいるキーフレームの右クリック。 */
export function KeyMenu() {
  const sel = useUi((s) => s.selectedKeys);
  const [pct, setPct] = useState(200);
  const n = sel?.keys.length ?? 0;
  return (
    <ContextMenuContent className="w-64">
      <ContextMenuGroup>
        <ContextMenuLabel className="text-xs text-muted-foreground">選択中のキーフレーム: {n} 個</ContextMenuLabel>
      </ContextMenuGroup>
      <ContextMenuSub>
        <ContextMenuSubTrigger disabled={n === 0}>キーフレーム補間</ContextMenuSubTrigger>
        <ContextMenuSubContent>
          {KEY_KINDS.map(([k, label]) => (
            <ContextMenuItem key={k} onClick={() => void keysOp("キーフレーム補間", { op: "kind", kind: k })}>
              {label}
              <ContextMenuShortcut>{shortcut(KIND_ACTION[k] ?? "")}</ContextMenuShortcut>
            </ContextMenuItem>
          ))}
        </ContextMenuSubContent>
      </ContextMenuSub>
      <ContextMenuItem disabled={n === 0} onClick={() => runAction("reverse_keys")}>
        時間反転<ContextMenuShortcut>{shortcut("reverse_keys")}</ContextMenuShortcut>
      </ContextMenuItem>
      <div className="flex items-center gap-2 px-2 py-1 text-sm" onPointerDown={(e) => e.stopPropagation()}>
        <span className="text-muted-foreground">時間伸縮</span>
        <NumberField value={pct} digits={0} min={-1000} max={1000} unit="%" onChange={(v) => setPct(v)} />
        <Button
          size="sm"
          variant="secondary"
          disabled={n === 0}
          onClick={() => sel && void runOp({ op: "scale_keys", layer: sel.layer, keys: sel.keys, percent: pct })}
        >
          適用
        </Button>
      </div>
      <ContextMenuSeparator />
      <ContextMenuItem disabled={n === 0} onClick={copyKeys}>
        コピー<ContextMenuShortcut>⌘C</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem onClick={() => void pasteKeys()}>
        再生ヘッドの位置に貼り付け<ContextMenuShortcut>⌘V</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem variant="destructive" disabled={n === 0} onClick={() => void deleteSelectedKey()}>
        削除<ContextMenuShortcut>{shortcut("delete")}</ContextMenuShortcut>
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

const KIND_ACTION: Record<string, string> = { easy_ease: "easy_ease", ease_in: "ease_in", ease_out: "ease_out", linear: "linear_keys", hold: "hold_keys" };

/** 操作に割り当てたショートカットの表記（最初のもの）。 */
export function shortcut(action: string): string {
  const c = chordsOf(action)[0];
  return c ? display(c) : "";
}

/** ビートグリッドの拍（小節の頭は濃く）。 */
function BeatGrid({ comp, view }: { comp: Composition; view: View }) {
  const g = comp.beat_grid as { bpm: number; offset: number; beats_per_bar: number; subdivision: number } | null | undefined;
  if (!g || g.bpm <= 0) return null;
  const rate = fps(comp.frame_rate);
  const beat = (60 / g.bpm) * rate;
  const ppb = (beat / view.span) * view.width;
  if (ppb < 4) return null;
  const off = (seconds(g.offset ?? 0) * rate) % beat;
  const marks: [number, boolean][] = [];
  const first = Math.ceil((view.start - off) / beat);
  for (let i = first; off + i * beat <= view.start + view.span && marks.length < 2000; i++) {
    marks.push([off + i * beat, ((i % (g.beats_per_bar || 4)) + (g.beats_per_bar || 4)) % (g.beats_per_bar || 4) === 0]);
  }
  return (
    <>
      {marks.map(([f, bar], i) => (
        <span key={i} className={`absolute top-0 w-px ${bar ? "h-3 bg-sky-400/80" : "h-1.5 bg-sky-400/40"}`} style={{ left: xOf(view, f) }} />
      ))}
    </>
  );
}

interface Marker {
  time: number;
  duration?: number;
  comment: string;
}

/** コンポのマーカー。クリックでその時刻へ、ドラッグで移動、ダブルクリックでコメント、右クリックで削除。 */
function MarkerLane({ comp, view }: { comp: Composition; view: View }) {
  const markers = (comp.markers as Marker[] | undefined) ?? [];
  const [editing, setEditing] = useState<number | null>(null);
  const fr = comp.frame_rate;
  const ppf = view.width / view.span;
  if (markers.length === 0) return null;
  const down = (e: React.PointerEvent, i: number) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    const f0 = timeToFrame(fr, markers[i].time);
    setFrame(f0);
    const startX = e.clientX;
    const move = (ev: PointerEvent) => {
      const f = Math.max(0, Math.round(f0 + (ev.clientX - startX) / ppf));
      editComp("マーカーを移動", (c) => {
        const m = (c.markers as Marker[] | undefined)?.[i];
        if (!m) return false;
        m.time = frameToTime(fr, f);
      }, true);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      editComp("マーカーを並べ直す", (c) => {
        const list = c.markers as Marker[] | undefined;
        if (!list || list.every((m, k) => k === 0 || list[k - 1].time <= m.time)) return false;
        list.sort((a, b) => a.time - b.time);
      });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div className="marker-lane">
      {markers.map((m, i) => (
        <ContextMenu key={i}>
          <ContextMenuTrigger
            render={<span />}
            className="marker"
            style={{ left: xOf(view, timeToFrame(fr, m.time)) }}
            title={m.comment || timecode(fr, timeToFrame(fr, m.time))}
            onPointerDown={(e: React.PointerEvent) => down(e, i)}
            onDoubleClick={() => setEditing(i)}
          >
            {editing === i ? (
              <input
                autoFocus
                className="marker-input"
                defaultValue={m.comment}
                onPointerDown={(e) => e.stopPropagation()}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter" || e.key === "Escape") (e.target as HTMLInputElement).blur();
                }}
                onBlur={(e) => {
                  const v = e.target.value;
                  setEditing(null);
                  if (v !== m.comment) editComp("マーカーのコメント", (c) => void ((c.markers as Marker[])[i].comment = v));
                }}
              />
            ) : (
              m.comment && <span className="marker-text">{m.comment}</span>
            )}
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem onClick={() => setEditing(i)}>コメントを編集</ContextMenuItem>
            <ContextMenuItem variant="destructive" onClick={() => editComp("マーカーを削除", (c) => void (c.markers as Marker[]).splice(i, 1))}>
              削除
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      ))}
    </div>
  );
}

/** シャイを隠す・動きのあるプロパティだけ・名前で絞り込む。 */
function TimelineFilters() {
  const hideShy = useUi((s) => s.hideShy);
  const onlyAnimated = useUi((s) => s.onlyAnimated);
  const search = useUi((s) => s.tlSearch);
  return (
    <>
      <input
        className="h-7 w-32 rounded-3xl bg-input/50 px-3 text-xs outline-none placeholder:text-muted-foreground/60"
        placeholder="レイヤーを検索"
        value={search}
        onChange={(e) => set({ tlSearch: e.target.value })}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") set({ tlSearch: "" });
        }}
      />
      <IconButton icon="shy" title="シャイのレイヤーを隠す" on={hideShy} onClick={() => set((s) => ({ hideShy: !s.hideShy }))} />
      <IconButton icon="diamond" title="動きのあるプロパティだけを表示" on={onlyAnimated} onClick={() => set((s) => ({ onlyAnimated: !s.onlyAnimated }))} />
    </>
  );
}

/** コンポの音の波形かスペクトログラム（音があるときだけ出す）。 */
function AudioLane({ comp, view }: { comp: Composition; view: View }) {
  const revision = useUi((s) => s.snap?.revision ?? 0);
  const [spectro, setSpectro] = useState(false);
  const [wave, setWave] = useState<[number, number][]>([]);
  const [tick, setTick] = useState(0);
  const canvas = useRef<HTMLCanvasElement>(null);
  const H = 48;
  useEffect(() => {
    let alive = true;
    const load = () =>
      void api.audioWave(comp.id, view.start, view.start + view.span, Math.max(1, Math.floor(view.width))).then((w) => {
        if (!alive) return;
        setWave(w);
        // ミックスが終わるまで、少し待って尋ね直す
        if (w.length === 0) setTimeout(() => alive && setTick((t) => t + 1), 800);
      });
    load();
    return () => {
      alive = false;
    };
  }, [comp.id, view.start, view.span, view.width, revision, tick]);
  useEffect(() => {
    if (!spectro) return;
    let alive = true;
    void api.spectrogram(comp.id).then((img) => {
      const c = canvas.current;
      if (!alive || !img || !c) return;
      // 表示範囲の部分だけを切り出して描く
      const total = Math.max(1, timeToFrame(comp.frame_rate, comp.duration));
      const off = document.createElement("canvas");
      off.width = img.width;
      off.height = img.height;
      off.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(img.rgba), img.width, img.height), 0, 0);
      c.width = Math.max(1, Math.floor(view.width));
      c.height = H;
      const ctx = c.getContext("2d");
      ctx?.drawImage(off, (view.start / total) * img.width, 0, (view.span / total) * img.width, img.height, 0, 0, c.width, H);
    });
    return () => {
      alive = false;
    };
  }, [spectro, comp.id, view.start, view.span, view.width, revision, wave.length > 0]);
  if (wave.length === 0 || wave.every(([a, b]) => a === 0 && b === 0)) return null;
  const mid = H / 2;
  return (
    <div className="track">
      <div className="track-row relative" style={{ height: H + 8 }}>
        <span className="absolute top-1 left-1 z-10 flex gap-1 text-[10px]">
          <button type="button" className={`rounded-full px-2 ${!spectro ? "bg-muted text-foreground" : "text-muted-foreground"}`} onClick={() => setSpectro(false)}>
            波形
          </button>
          <button type="button" className={`rounded-full px-2 ${spectro ? "bg-muted text-foreground" : "text-muted-foreground"}`} onClick={() => setSpectro(true)}>
            スペクトログラム
          </button>
        </span>
        {spectro ? (
          <canvas ref={canvas} className="absolute top-1" style={{ left: view.left, width: view.width, height: H }} />
        ) : (
          <svg className="absolute top-1" style={{ left: view.left }} width={view.width} height={H}>
            {wave.map(([lo, hi], x) => (
              <line key={x} x1={x + 0.5} x2={x + 0.5} y1={mid - hi * (mid - 2)} y2={mid - lo * (mid - 2) + 1} stroke="var(--audio)" />
            ))}
          </svg>
        )}
      </div>
    </div>
  );
}

/** いまのフレームの音の大きさ（左右、-48dB〜0dB）。 */
function LevelMeter({ comp }: { comp: Composition }) {
  const frame = useUi((s) => s.frame);
  const [peak, setPeak] = useState<[number, number] | null>(null);
  useEffect(() => {
    let alive = true;
    void api.audioPeak(comp.id, frame).then((p) => alive && setPeak(p));
    return () => {
      alive = false;
    };
  }, [comp.id, frame]);
  if (!peak) return null;
  const db = (v: number) => (v > 0 ? 20 * Math.log10(v) : -Infinity);
  const bar = (v: number) => {
    const d = db(v);
    const level = Math.max(0, Math.min(1, (d + 48) / 48));
    const color = d > -1 ? "bg-red-500" : d > -6 ? "bg-amber-400" : "bg-emerald-500";
    return (
      <span className="block h-1 w-16 overflow-hidden rounded-full bg-muted">
        <span className={`block h-full ${color}`} style={{ width: `${level * 100}%` }} />
      </span>
    );
  };
  const fmt = (v: number) => (Number.isFinite(db(v)) ? db(v).toFixed(1) : "-∞");
  return (
    <span className="flex flex-col gap-0.5" title={`L ${fmt(peak[0])} dB　R ${fmt(peak[1])} dB`}>
      {bar(peak[0])}
      {bar(peak[1])}
    </span>
  );
}
