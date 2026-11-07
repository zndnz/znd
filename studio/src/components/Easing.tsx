// イージング。タイムラインで選んだキーフレームの区間に、カーブ（ベジェ・振動・バウンス・階段）を付ける。
// - 一覧: よく使うカーブを、形の分かる小さなグラフで並べる。押すと選択中の区間に付く。
// - 編集: いまのカーブを、ハンドルのドラッグ・数値・`cubic-bezier(...)` の文字で直せる。
//   「自動で適用」を入れておくと、触るたびに選択中の区間へ反映する。
// - 補間の種類（リニア・イージーイーズ・停止など）、スポイト（選択の区間のカーブを拾う）、カーブを外す。

import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { type EaseCurve, type EaseDir, type EasePreset, KEY_KINDS } from "../model";
import { currentComp, get, keysOp, set as setUi, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";
import { Choice, NumberField, Row } from "./controls";

let presetsCache: Promise<EasePreset[]> | null = null;
const loadPresets = () => (presetsCache ??= api.curvePresets());

/** 編集中のカーブ（パネルを閉じても覚えておく）。 */
let remembered: EaseCurve = { type: "bezier", x1: 0.33, y1: 0, x2: 0.67, y2: 1 };

const css = (c: EaseCurve) => (c.type === "bezier" ? `cubic-bezier(${[c.x1, c.y1, c.x2, c.y2].map((v) => +v.toFixed(3)).join(", ")})` : "");
const same = (a: EaseCurve, b: EaseCurve) => JSON.stringify(a) === JSON.stringify(b);

/** カーブの値の列（ベジェはその場で計算し、それ以外は Rust に頼む）。 */
function useSamples(c: EaseCurve, n = 64): number[] {
  const [s, setS] = useState<number[]>([]);
  useEffect(() => {
    if (c.type === "bezier") return setS([]);
    let alive = true;
    void api.curveSamples(c, n).then((v) => alive && setS(v));
    return () => {
      alive = false;
    };
  }, [JSON.stringify(c), n]);
  return s;
}

/** 0..1 の時間 × 値の点列を SVG のパスにする。値の範囲は lo..hi。 */
function pathOf(c: EaseCurve, samples: number[], w: number, h: number, lo: number, hi: number, pad: number) {
  const X = (u: number) => pad + u * (w - pad * 2);
  const Y = (v: number) => h - pad - ((v - lo) / (hi - lo)) * (h - pad * 2);
  if (c.type === "bezier") {
    const pts: string[] = [];
    for (let i = 0; i <= 64; i++) {
      const s = i / 64;
      const b = (p1: number, p2: number) => 3 * (1 - s) * (1 - s) * s * p1 + 3 * (1 - s) * s * s * p2 + s * s * s;
      pts.push(`${X(b(c.x1, c.x2)).toFixed(1)},${Y(b(c.y1, c.y2)).toFixed(1)}`);
    }
    return "M" + pts.join("L");
  }
  return "M" + samples.map((v, i) => `${X(i / (samples.length - 1)).toFixed(1)},${Y(v).toFixed(1)}`).join("L");
}

function range(c: EaseCurve, samples: number[]): [number, number] {
  let lo = 0;
  let hi = 1;
  if (c.type === "bezier") (lo = Math.min(lo, c.y1, c.y2)), (hi = Math.max(hi, c.y1, c.y2));
  for (const v of samples) (lo = Math.min(lo, v)), (hi = Math.max(hi, v));
  return [lo, hi];
}

function Thumb({ preset, on, onPick }: { preset: EasePreset; on: boolean; onPick: () => void }) {
  const [lo, hi] = range(preset.curve, preset.samples);
  return (
    <button
      type="button"
      title={preset.name}
      onClick={onPick}
      className={cn("flex flex-col items-center gap-1 rounded-2xl p-1.5 text-[10px] text-muted-foreground hover:bg-muted", on && "bg-muted text-foreground")}
    >
      <svg viewBox="0 0 56 40" className="h-10 w-14">
        <path d={pathOf(preset.curve, preset.samples, 56, 40, lo, hi, 4)} fill="none" stroke="currentColor" strokeWidth={1.6} className="text-foreground" />
      </svg>
      <span className="w-14 truncate">{preset.name}</span>
    </button>
  );
}

/** ベジェのハンドルをドラッグできるカーブの図。 */
function Editor({ curve, onChange }: { curve: EaseCurve; onChange: (c: EaseCurve, done: boolean) => void }) {
  const W = 280;
  const H = 200;
  const PAD = 22;
  const samples = useSamples(curve);
  const [lo, hi] = range(curve, samples);
  const svg = useRef<SVGSVGElement>(null);
  const X = (u: number) => PAD + u * (W - PAD * 2);
  const Y = (v: number) => H - PAD - ((v - lo) / (hi - lo)) * (H - PAD * 2);
  const drag = (which: 1 | 2) => (e: React.PointerEvent) => {
    if (curve.type !== "bezier") return;
    (e.target as Element).setPointerCapture(e.pointerId);
    const rect = svg.current!.getBoundingClientRect();
    const at = (ev: PointerEvent | React.PointerEvent): EaseCurve => {
      const u = Math.min(1, Math.max(0, (((ev.clientX - rect.left) / rect.width) * W - PAD) / (W - PAD * 2)));
      const v = lo + ((H - PAD - ((ev.clientY - rect.top) / rect.height) * H) / (H - PAD * 2)) * (hi - lo);
      const r = (n: number) => Math.round(n * 1000) / 1000;
      return which === 1 ? { ...curve, x1: r(u), y1: r(v) } : { ...curve, x2: r(u), y2: r(v) };
    };
    const move = (ev: PointerEvent) => onChange(at(ev), false);
    const up = (ev: PointerEvent) => {
      onChange(at(ev), true);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <svg ref={svg} viewBox={`0 0 ${W} ${H}`} className="w-full rounded-3xl bg-input/40">
      <line x1={X(0)} y1={Y(0)} x2={X(1)} y2={Y(0)} className="stroke-border" />
      <line x1={X(0)} y1={Y(1)} x2={X(1)} y2={Y(1)} className="stroke-border" strokeDasharray="3 3" />
      <line x1={X(0)} y1={Y(0)} x2={X(1)} y2={Y(1)} className="stroke-border" strokeDasharray="3 3" />
      {curve.type === "bezier" && (
        <>
          <line x1={X(0)} y1={Y(0)} x2={X(curve.x1)} y2={Y(curve.y1)} className="stroke-muted-foreground" />
          <line x1={X(1)} y1={Y(1)} x2={X(curve.x2)} y2={Y(curve.y2)} className="stroke-muted-foreground" />
        </>
      )}
      <path d={pathOf(curve, samples, W, H, lo, hi, PAD)} fill="none" strokeWidth={2.2} className="stroke-foreground" />
      {curve.type === "bezier" && (
        <>
          <circle cx={X(curve.x1)} cy={Y(curve.y1)} r={7} className="cursor-grab fill-primary" onPointerDown={drag(1)} />
          <circle cx={X(curve.x2)} cy={Y(curve.y2)} r={7} className="cursor-grab fill-primary" onPointerDown={drag(2)} />
        </>
      )}
    </svg>
  );
}

const DIRS: [EaseDir, string][] = [
  ["in", "イン"],
  ["out", "アウト"],
  ["in_out", "イン・アウト"],
];

function defaults(type: EaseCurve["type"], from: EaseCurve): EaseCurve {
  const dir = "dir" in from ? from.dir : "out";
  switch (type) {
    case "bezier":
      return { type, x1: 0.33, y1: 0, x2: 0.67, y2: 1 };
    case "elastic":
      return { type, dir, amplitude: 1, frequency: 3, decay: 6 };
    case "bounce":
      return { type, dir, bounces: 3, restitution: 0.5 };
    case "steps":
      return { type, count: 4 };
  }
}

export function EasingPanel() {
  const sel = useUi((s) => s.selectedKeys);
  const [presets, setPresets] = useState<EasePreset[]>([]);
  const [curve, setCurveState] = useState<EaseCurve>(remembered);
  const [live, setLive] = useState(true);
  const [text, setText] = useState(css(remembered));
  useEffect(() => void loadPresets().then(setPresets), []);
  const groups = useMemo(() => [...new Set(presets.map((p) => p.group))], [presets]);
  const count = sel?.keys.length ?? 0;

  const apply = (c: EaseCurve | null, coalesce = false) => {
    if (count === 0) return setUi({ status: "タイムラインでキーフレームを選んでください" });
    void keysOp(c ? "イージング" : "イージングを外す", { op: "curve", curve: c }, coalesce);
  };
  const setCurve = (c: EaseCurve, commit: boolean, push = live) => {
    remembered = c;
    setCurveState(c);
    setText(css(c));
    if (push && count > 0) apply(c, !commit);
  };
  const pick = async () => {
    const s = get();
    const layer = currentComp()?.layers.find((l) => l.id === s.selectedKeys?.layer);
    if (!layer || !s.selectedKeys) return;
    const c = await api.curveOf(layer, s.selectedKeys.keys);
    if (c) setCurve(c, true, false);
    else setUi({ status: "選んだ区間にはカーブが付いていません" });
  };

  return (
    <div className="flex flex-col gap-3">
      <p className="text-xs text-muted-foreground">
        {count > 0 ? `${count} 個のキーフレームを選んでいます。カーブを選ぶとその区間に付きます。` : "タイムラインでキーフレームを選ぶ（Shift で追加）と、その区間に付けられます。"}
      </p>

      <ToggleGroup
        className="flex-wrap"
        value={[]}
        onValueChange={(v) => v[0] && void keysOp("キーフレーム補間", { op: "kind", kind: v[0] as never })}
      >
        {KEY_KINDS.map(([k, label]) => (
          <ToggleGroupItem key={k} value={k} size="sm" disabled={count === 0} className="px-2.5 text-xs">
            {label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>

      <Editor curve={curve} onChange={(c, done) => setCurve(c, done)} />

      <Row label="種類">
        <Choice
          value={curve.type}
          options={[
            ["bezier", "ベジェ"],
            ["elastic", "振動"],
            ["bounce", "バウンス"],
            ["steps", "階段"],
          ]}
          onChange={(t) => setCurve(defaults(t as EaseCurve["type"], curve), true)}
        />
      </Row>
      {curve.type === "bezier" && (
        <>
          <div className="grid grid-cols-4 gap-1.5">
            {(["x1", "y1", "x2", "y2"] as const).map((k) => (
              <NumberField
                key={k}
                value={curve[k]}
                step={0.005}
                digits={3}
                min={k.startsWith("x") ? 0 : -4}
                max={k.startsWith("x") ? 1 : 5}
                onChange={(v, d) => setCurve({ ...curve, [k]: v }, !d)}
              />
            ))}
          </div>
          <Input
            className="h-8 font-mono text-xs"
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") void api.parseCurve(text).then((c) => (c ? setCurve(c, true) : setUi({ status: "読めない書式です" })));
            }}
            placeholder="cubic-bezier(0.33, 0, 0.67, 1)"
          />
        </>
      )}
      {(curve.type === "elastic" || curve.type === "bounce") && (
        <Row label="向き">
          <Choice value={curve.dir} options={DIRS} onChange={(d) => setCurve({ ...curve, dir: d as EaseDir }, true)} />
        </Row>
      )}
      {curve.type === "elastic" && (
        <>
          <Row label="強さ">
            <NumberField value={curve.amplitude} step={0.02} digits={2} min={0} max={10} onChange={(v, d) => setCurve({ ...curve, amplitude: v }, !d)} />
          </Row>
          <Row label="振動の回数">
            <NumberField value={curve.frequency} step={0.05} digits={2} min={0.25} max={40} onChange={(v, d) => setCurve({ ...curve, frequency: v }, !d)} />
          </Row>
          <Row label="減衰">
            <NumberField value={curve.decay} step={0.1} digits={1} min={0} max={60} onChange={(v, d) => setCurve({ ...curve, decay: v }, !d)} />
          </Row>
        </>
      )}
      {curve.type === "bounce" && (
        <>
          <Row label="跳ねる回数">
            <NumberField value={curve.bounces} step={0.1} digits={0} min={1} max={20} onChange={(v, d) => setCurve({ ...curve, bounces: Math.round(v) }, !d)} />
          </Row>
          <Row label="反発">
            <NumberField value={curve.restitution} step={0.01} digits={2} min={0.05} max={0.95} onChange={(v, d) => setCurve({ ...curve, restitution: v }, !d)} />
          </Row>
        </>
      )}
      {curve.type === "steps" && (
        <Row label="段数">
          <NumberField value={curve.count} step={0.1} digits={0} min={1} max={1000} onChange={(v, d) => setCurve({ ...curve, count: Math.round(v) }, !d)} />
        </Row>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={count === 0} onClick={() => apply(curve)}>
          選択に適用
        </Button>
        <Button size="sm" variant="secondary" disabled={count === 0} onClick={() => void pick()}>
          スポイト
        </Button>
        <Button size="sm" variant="ghost" disabled={count === 0} onClick={() => apply(null)}>
          外す
        </Button>
        <label className="ml-auto flex items-center gap-2 text-xs text-muted-foreground">
          <Switch checked={live} onCheckedChange={setLive} />
          自動で適用
        </label>
      </div>

      {groups.map((g) => (
        <div key={g}>
          <div className="px-1 pb-1 text-xs text-muted-foreground">{g}</div>
          <div className="grid grid-cols-4 gap-0.5">
            {presets
              .filter((p) => p.group === g)
              .map((p) => (
                <Thumb key={p.name} preset={p} on={same(p.curve, curve)} onPick={() => setCurve(p.curve, true, true)} />
              ))}
          </div>
        </div>
      ))}
    </div>
  );
}

/** 選択中の区間に、いまのカーブを付ける（E キー）。 */
export function applyRememberedEase() {
  if (get().selectedKeys?.keys.length) void keysOp("イージング", { op: "curve", curve: remembered });
}
