// グラフエディター（値のグラフ・速度のグラフ）とスクリプトのウィンドウ。
// グラフは選んでいるレイヤーの 1 つのプロパティを描き、キーをドラッグで時間（フレーム単位）と値（1 次元のとき）を動かす。
// 値の計算は Rust（props::graph）が行う。

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../api";
import { KEY_KINDS, fps, frameToTime, type KeyAddr, seconds, timeToFrame } from "../model";
import { currentComp, get, keysOp, propOp, run, selectedLayer, selectKey, set, setFrame, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";
import { IconButton, NumberField } from "./controls";

const COLORS = ["#ff6e6e", "#6edc82", "#78a0ff", "#e6e678"];

export function openGraph(path: string) {
  set({ graphPath: path, graphOpen: true });
}

export function GraphWindow() {
  const open = useUi((s) => s.graphOpen);
  const path = useUi((s) => s.graphPath);
  const layer = useUi(selectedLayer);
  const comp = useUi(currentComp);
  const frame = useUi((s) => s.frame);
  const sel = useUi((s) => s.selectedKeys);
  const [speed, setSpeed] = useState(false);
  const [view, setView] = useState<[number, number] | null>(null);
  const [viewFor, setViewFor] = useState("");
  const [pos, setPos] = useState({ x: 160, y: 120 });
  const [size, setSize] = useState({ w: 680, h: 300 });
  const [easeIn, setEaseIn] = useState<[number, number]>([0, 33.3]);
  const [easeOut, setEaseOut] = useState<[number, number]>([0, 33.3]);
  const [graph, setGraph] = useState<{ series: number[][]; keys: [number, number[]][]; dims: number } | null>(null);
  const area = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(600);
  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [open]);

  const dur = comp ? Math.max(1e-6, seconds(comp.duration)) : 1;
  const rate = comp ? fps(comp.frame_rate) : 30;
  const minSpan = Math.min(4 / rate, dur);
  // 別のプロパティを開いたら、キーが収まる範囲から始める
  const key = `${layer?.id}/${path}`;
  const [v0, v1] = view && viewFor === key ? view : [0, dur];
  useEffect(() => {
    if (!open || !layer || !path) return;
    let alive = true;
    void api.graph(layer, path, v0, v1, Math.max(2, Math.floor(width)), speed, rate).then((g) => {
      if (!alive) return;
      setGraph(g);
      if (g && viewFor !== key) {
        const ks = g.keys.map((k) => seconds(k[0]));
        if (ks.length >= 2 && ks[ks.length - 1] > ks[0]) {
          const m = Math.max((ks[ks.length - 1] - ks[0]) * 0.08, minSpan);
          setView([ks[0] - m, ks[ks.length - 1] + m]);
        } else setView([0, dur]);
        setViewFor(key);
      }
    });
    return () => {
      alive = false;
    };
  }, [open, layer, path, v0, v1, width, speed]);

  if (!open || !comp) return null;
  const h = Math.max(120, size.h - 120);
  const all = graph?.series.flat() ?? [];
  let lo = Math.min(...all, Infinity);
  let hi = Math.max(...all, -Infinity);
  if (!Number.isFinite(lo)) [lo, hi] = [0, 1];
  if (hi - lo < 1e-6) [lo, hi] = [lo - 1, hi + 1];
  else [lo, hi] = [lo - (hi - lo) * 0.1, hi + (hi - lo) * 0.1];
  const xOf = (s: number) => ((s - v0) / (v1 - v0)) * width;
  const yOf = (v: number) => h - ((v - lo) / (hi - lo)) * h;
  const secAt = (x: number) => v0 + (x / width) * (v1 - v0);
  const fr = comp.frame_rate;
  const now = seconds(frameToTime(fr, frame));
  const targets: KeyAddr[] = sel && sel.layer === layer?.id ? sel.keys.filter((k) => k.path === path) : [];

  const setRange = (a: number, b: number) => {
    const span = Math.max(minSpan, Math.min(dur * 1.2, b - a));
    const lim = -dur * 0.1;
    const s0 = Math.max(lim, Math.min(a, lim + dur * 1.2 - span));
    setView([s0, s0 + span]);
    setViewFor(key);
  };
  const onWheel = (e: React.WheelEvent) => {
    const r = area.current!.getBoundingClientRect();
    const at = secAt(e.clientX - r.left);
    if (!e.shiftKey && Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
      const f = Math.exp(e.deltaY * 0.004);
      const span = (v1 - v0) * f;
      const ratio = (at - v0) / (v1 - v0);
      setRange(at - ratio * span, at - ratio * span + span);
    } else {
      const d = ((e.shiftKey ? e.deltaY : e.deltaX) / width) * (v1 - v0);
      setRange(v0 + d, v1 + d);
    }
  };
  const onDown = (e: React.PointerEvent) => {
    if (!layer || !path || !graph) return;
    const r = area.current!.getBoundingClientRect();
    const panning = e.button === 1 || e.altKey;
    const start = { x: e.clientX, v0, v1 };
    const near = graph.keys.map((k) => k[0]).find((t) => Math.abs(xOf(seconds(t)) - (e.clientX - r.left)) < 10);
    if (!panning) {
      if (near !== undefined) {
        selectKey(layer.id, { path, time: near }, e.shiftKey || e.metaKey);
        setFrame(timeToFrame(fr, near));
      } else setFrame(Math.round(secAt(e.clientX - r.left) * rate));
    }
    let grabbed = near;
    const move = (ev: PointerEvent) => {
      if (panning) {
        const d = (-(ev.clientX - start.x) / width) * (start.v1 - start.v0);
        return setRange(start.v0 + d, start.v1 + d);
      }
      if (grabbed === undefined || layer.locked) return;
      const f = Math.max(0, Math.min(Math.round(secAt(ev.clientX - r.left) * rate), Math.max(0, timeToFrame(fr, comp.duration) - 1)));
      const to = frameToTime(fr, f);
      if (to !== grabbed && !graph.keys.some((k) => k[0] === to)) {
        const from = grabbed;
        grabbed = to;
        void propOp("グラフ編集", layer.id, path, { op: "move_key", from, to }, true);
        set((s) => (s.selectedKeys?.layer === layer.id ? { selectedKeys: { layer: layer.id, keys: s.selectedKeys.keys.map((k) => (k.path === path && k.time === from ? { path, time: to } : k)) } } : {}));
        setFrame(f);
      } else if (!speed && graph.dims === 1) {
        // 1 次元のプロパティは、上下ドラッグで値を変えられる
        const v = lo + ((h - (ev.clientY - r.top)) / h) * (hi - lo);
        setFrame(timeToFrame(fr, grabbed));
        void propOp("グラフ編集", layer.id, path, { op: "set", value: [Math.round(v * 100) / 100] }, true);
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  const ease = (which: "in" | "out") => {
    if (!targets.length) return set({ status: "キーフレームを選択してください" });
    void keysOp("イーズ", { op: "ease", ease_in: which === "in" ? easeIn : null, ease_out: which === "out" ? easeOut : null });
  };
  const drag = (e: React.PointerEvent, resize: boolean) => {
    const st = { x: e.clientX, y: e.clientY, pos, size };
    const move = (ev: PointerEvent) =>
      resize
        ? setSize({ w: Math.max(420, st.size.w + ev.clientX - st.x), h: Math.max(240, st.size.h + ev.clientY - st.y) })
        : setPos({ x: st.pos.x + ev.clientX - st.x, y: Math.max(0, st.pos.y + ev.clientY - st.y) });
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // 時間の目盛り
  const pps = width / (v1 - v0);
  const steps = [1 / rate, 2 / rate, 5 / rate, 10 / rate, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300];
  const step = steps.find((s) => s * pps >= 70) ?? 600;
  const ticks: number[] = [];
  for (let t = Math.ceil(Math.max(0, v0) / step) * step; t <= Math.min(v1, dur) + 1e-9 && ticks.length < 200; t += step) ticks.push(t);

  return (
    <div className="fixed z-40 flex flex-col overflow-hidden rounded-3xl bg-popover text-sm shadow-xl ring-1 ring-foreground/10" style={{ left: pos.x, top: pos.y, width: size.w, height: size.h }}>
      <div className="flex h-10 shrink-0 cursor-grab items-center gap-2 pr-1.5 pl-4" onPointerDown={(e) => drag(e, false)}>
        <span className="flex-1 truncate font-semibold">グラフエディター{layer && path ? `: ${layer.name} › ${path}` : ""}</span>
        <Button variant={speed ? "ghost" : "secondary"} size="sm" className="h-7 text-xs" onPointerDown={(e) => e.stopPropagation()} onClick={() => setSpeed(false)}>
          値
        </Button>
        <Button variant={speed ? "secondary" : "ghost"} size="sm" className="h-7 text-xs" onPointerDown={(e) => e.stopPropagation()} onClick={() => setSpeed(true)}>
          速度
        </Button>
        <IconButton icon="close" title="閉じる" size="icon-xs" onClick={() => set({ graphOpen: false })} />
      </div>
      {!layer || !path ? (
        <p className="px-4 text-muted-foreground">プロパティの行のグラフのボタンを押すと、ここにグラフが出ます。</p>
      ) : (
        <>
          <div className="flex shrink-0 flex-wrap items-center gap-1 px-4 pb-1 text-xs">
            <span className="text-muted-foreground">対象のキー {targets.length} 個:</span>
            {KEY_KINDS.map(([k, label]) => (
              <Button key={k} variant="ghost" size="sm" className="h-6 px-1.5 text-xs" disabled={!targets.length} onClick={() => void keysOp("キーフレーム補間", { op: "kind", kind: k })}>
                {label}
              </Button>
            ))}
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-1.5 px-4 pb-1 text-xs">
            <span className="text-muted-foreground">入る側</span>
            <NumberField className="max-w-20" value={easeIn[0]} onChange={(v) => setEaseIn([v, easeIn[1]])} />
            <NumberField className="max-w-20" value={easeIn[1]} min={0.1} max={100} unit="%" onChange={(v) => setEaseIn([easeIn[0], v])} />
            <Button variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => ease("in")}>
              適用
            </Button>
            <span className="pl-2 text-muted-foreground">出る側</span>
            <NumberField className="max-w-20" value={easeOut[0]} onChange={(v) => setEaseOut([v, easeOut[1]])} />
            <NumberField className="max-w-20" value={easeOut[1]} min={0.1} max={100} unit="%" onChange={(v) => setEaseOut([easeOut[0], v])} />
            <Button variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => ease("out")}>
              適用
            </Button>
            <span className="flex-1" />
            <Button variant="ghost" size="sm" className="h-6 px-1.5 text-xs" onClick={() => setRange(0, dur)}>
              全体
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-1.5 text-xs"
              disabled={(graph?.keys.length ?? 0) < 2}
              onClick={() => {
                const ks = graph!.keys.map((k) => seconds(k[0]));
                const m = Math.max((ks[ks.length - 1] - ks[0]) * 0.08, minSpan);
                setRange(ks[0] - m, ks[ks.length - 1] + m);
              }}
            >
              キーに合わせる
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-1.5 text-xs"
              onClick={() => {
                const s = get();
                const total = Math.max(1, timeToFrame(fr, comp.duration));
                const span = total / s.tlZoom;
                setRange(s.tlStart / rate, (s.tlStart + span) / rate);
              }}
            >
              タイムラインと同じ
            </Button>
          </div>
          <div ref={area} className="relative mx-4 mb-4 flex-1 overflow-hidden rounded-2xl bg-black/40" onWheel={onWheel} onPointerDown={onDown}>
            <svg width={width} height={h} className="absolute inset-0">
              {ticks.map((t) => (
                <g key={t}>
                  <line x1={xOf(t)} y1={0} x2={xOf(t)} y2={h} stroke="white" strokeOpacity={0.05} />
                  <text x={xOf(t) + 2} y={12} fontSize={9} fill="#777" fontFamily="monospace">
                    {step >= 1 ? `${Math.floor(t / 60)}:${String(Math.round(t) % 60).padStart(2, "0")}` : `${t.toFixed(2)}`}
                  </text>
                </g>
              ))}
              {xOf(0) > 0 && <rect x={0} y={0} width={xOf(0)} height={h} fill="black" fillOpacity={0.35} />}
              {xOf(dur) < width && <rect x={xOf(dur)} y={0} width={width - xOf(dur)} height={h} fill="black" fillOpacity={0.35} />}
              {graph &&
                Array.from({ length: Math.min(4, graph.dims) }, (_, d) => (
                  <polyline key={d} fill="none" stroke={COLORS[d]} strokeWidth={1.5} points={graph.series.map((v, c) => `${c},${yOf(v[d] ?? 0)}`).join(" ")} />
                ))}
              {graph?.keys.map(([t, vals]) => (
                <g key={t}>
                  {!speed &&
                    vals.slice(0, 4).map((v, d) => (
                      <circle key={d} cx={xOf(seconds(t))} cy={yOf(v)} r={targets.some((k) => k.time === t) ? 5 : 4} fill={t === frameToTime(fr, frame) ? "white" : COLORS[d]} />
                    ))}
                  <line x1={xOf(seconds(t))} y1={h - 6} x2={xOf(seconds(t))} y2={h} stroke="white" strokeWidth={2} />
                </g>
              ))}
              <line x1={xOf(now)} y1={0} x2={xOf(now)} y2={h} stroke="#ff5a5a" />
              <text x={4} y={h - 4} fontSize={10} fill="#888" fontFamily="monospace">
                {lo.toFixed(1)}
              </text>
              <text x={4} y={24} fontSize={10} fill="#888" fontFamily="monospace">
                {hi.toFixed(1)}
              </text>
            </svg>
          </div>
          <p className="-mt-3 px-4 pb-1 text-[11px] text-muted-foreground/70">ホイール: 拡大縮小 / Shift+ホイール・横スクロール・中ボタンか Alt のドラッグ: 移動</p>
        </>
      )}
      <span className="absolute right-1 bottom-1 size-3 cursor-nwse-resize" onPointerDown={(e) => (e.stopPropagation(), drag(e, true))} />
    </div>
  );
}

// ---------- スクリプト ----------

let scriptText = "// project を書き換えます。例:\nforEachLayer(function (l) { l.shy = false; });\n";

export function ScriptDialog() {
  const open = useUi((s) => s.dialog === "script");
  const [text, setText] = useState(scriptText);
  const [logs, setLogs] = useState<string[]>([]);
  const update = (t: string) => setText((scriptText = t));
  return (
    <Dialog open={open} onOpenChange={(o) => !o && set({ dialog: null })}>
      <DialogContent className="flex max-h-[86vh] flex-col sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>スクリプト</DialogTitle>
          <DialogDescription>JavaScript でプロジェクト全体を書き換えます。`project` がプロジェクトファイルと同じ構造のオブジェクトです。</DialogDescription>
        </DialogHeader>
        <Textarea className="min-h-64 font-mono text-xs" value={text} onChange={(e) => update(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
        <p className="text-xs text-muted-foreground">
          使える補助関数: comp(名前) / layer(コンポ, 名前) / forEachLayer(fn) / key(秒, 値, 'ease') / setKeys(プロパティ, [..]) / flicks(秒) / log(..)
        </p>
        {logs.length > 0 && <div className="max-h-32 overflow-y-auto font-mono text-xs whitespace-pre-wrap text-muted-foreground select-text">{logs.join("\n")}</div>}
        <div className="flex justify-end gap-2">
          <Button
            variant="ghost"
            onClick={() =>
              void api
                .loadScript()
                .then((t) => t !== null && update(t))
                .catch((e) => set({ status: String(e) }))
            }
          >
            ファイルから読み込む…
          </Button>
          <Button
            onClick={() =>
              void run(async () => {
                try {
                  const [l, snap] = await api.runScript(text);
                  setLogs(l);
                  set({ status: l.length ? `スクリプト: ${l.join(" / ")}` : "スクリプトを実行しました" });
                  return snap;
                } catch (e) {
                  set({ status: `スクリプトエラー: ${e}` });
                  return null;
                }
              })
            }
          >
            実行
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
