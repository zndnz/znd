// プロパティ 1 行（インスペクタ）。どのプロパティ（トランスフォーム・エフェクト・マスク・テキスト・シェイプ・
// カメラ・ライト）も同じ形で出し、◆ でキーフレームを打てる。値の書き換えは Rust の `prop_edit` で行う。

import { useEffect, useState } from "react";
import { api } from "../api";
import type { Layer, PropInfo } from "../model";
import { nowTime, propOp, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { NumberField } from "./controls";
import { Icon } from "./icons";

/** レイヤーの全プロパティ（いま画面に出ているレイヤーと再生ヘッドの時刻で）。 */
export function useLayerProps(layer: Layer | undefined): PropInfo[] {
  const frame = useUi((s) => s.frame);
  const [props, setProps] = useState<PropInfo[]>([]);
  useEffect(() => {
    if (!layer) return setProps([]);
    let alive = true;
    void api.layerProps(layer, nowTime()).then((p) => alive && setProps(p));
    return () => {
      alive = false;
    };
  }, [layer, frame]);
  return props;
}

/** レイヤーの全プロパティのキーフレーム（時刻に依存しないので、レイヤーが変わったときだけ取り直す）。 */
export function useLayerKeys(layer: Layer): PropInfo[] {
  const [props, setProps] = useState<PropInfo[]>([]);
  useEffect(() => {
    let alive = true;
    void api.layerProps(layer, 0).then((p) => alive && setProps(p));
    return () => {
      alive = false;
    };
  }, [layer]);
  return props;
}

const toHex = (c: number[]) =>
  "#" + c.slice(0, 3).map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, "0")).join("");
const fromHex = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);

/** 単位と刻み（名前から決める）。 */
function unitOf(path: string): { unit: string; step: number; digits: number; min?: number; max?: number } {
  const last = path.split(".").pop() ?? "";
  if (last.includes("rotation") || last.includes("angle")) return { unit: "°", step: 1, digits: 1 };
  if (last === "opacity" || last.endsWith("_opacity")) return { unit: "%", step: 0.5, digits: 1, min: 0, max: 100 };
  if (last === "scale") return { unit: "%", step: 1, digits: 1 };
  if (path === "audio.level") return { unit: "dB", step: 0.1, digits: 1 };
  return { unit: "", step: 1, digits: 2 };
}

export function PropRow({ layer, prop, label }: { layer: Layer; prop: PropInfo; label?: string }) {
  const t = useUi(() => nowTime());
  const keyHere = prop.keys.some((k) => k.time === t);
  const { unit, step, digits, min, max } = unitOf(prop.path);
  const set = (vals: number[], dragging: boolean) => void propOp(label ?? prop.label, layer.id, prop.path, { op: "set", value: vals }, dragging);
  const color = prop.vals.length === 4 && !prop.path.startsWith("camera");

  return (
    <div className="flex min-h-8 items-center gap-1.5">
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label={prop.animated ? "この時刻のキーフレーム" : "アニメーションを始める"}
        title={prop.animated ? "この時刻にキーフレームを打つ／外す（右クリックでアニメーションを止める）" : "アニメーションを始める（今の値でキーフレームを打つ）"}
        className={cn("shrink-0", prop.animated ? "text-orange-400" : "text-muted-foreground/50")}
        onClick={() =>
          void propOp("キーフレーム", layer.id, prop.path, prop.animated ? { op: keyHere ? "remove_key" : "add_key" } : { op: "toggle_animation" })
        }
        onContextMenu={(e) => {
          e.preventDefault();
          if (prop.animated) void propOp("アニメーションを止める", layer.id, prop.path, { op: "toggle_animation" });
        }}
      >
        <Icon name="diamond" size={12} filled={keyHere} />
      </Button>
      <span className="w-24 shrink-0 truncate text-sm text-muted-foreground" title={label ?? prop.label}>
        {label ?? prop.label}
      </span>
      <span className="flex min-w-0 flex-1 items-center gap-1.5">
        {prop.is_path ? (
          <span className="text-sm text-muted-foreground">パス（ビューアで編集）</span>
        ) : color ? (
          <>
            <input
              type="color"
              className="h-8 w-12 rounded-3xl"
              value={toHex(prop.vals)}
              onChange={(e) => set([...fromHex(e.target.value), prop.vals[3]], true)}
            />
            <NumberField value={prop.vals[3] * 100} unit="%" min={0} max={100} digits={0} onChange={(v, d) => set([...prop.vals.slice(0, 3), v / 100], d)} />
          </>
        ) : (
          prop.vals.map((v, i) => (
            <NumberField
              key={i}
              value={v}
              unit={unit}
              step={step}
              digits={digits}
              min={min}
              max={max}
              onChange={(n, d) => set(prop.vals.map((x, j) => (j === i ? n : x)), d)}
            />
          ))
        )}
      </span>
      <ExprButton layer={layer} prop={prop} />
    </div>
  );
}

/** エクスプレッションの編集（あるときは「=」を明るくする）。 */
function ExprButton({ layer, prop }: { layer: Layer; prop: PropInfo }) {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState(prop.expr ?? "");
  useEffect(() => setText(prop.expr ?? ""), [prop.expr, open]);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="エクスプレッション"
            className={cn("shrink-0 font-mono text-xs", prop.expr ? "text-foreground" : "text-muted-foreground/40")}
          />
        }
      >
        =
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80">
        <div className="text-sm font-medium">エクスプレッション: {prop.label}</div>
        <Textarea
          className="min-h-24 font-mono text-xs"
          value={text}
          placeholder="例: wiggle(2, 30)"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.stopPropagation()}
        />
        <div className="flex justify-end gap-2">
          {prop.expr && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => (setOpen(false), void propOp("エクスプレッションを外す", layer.id, prop.path, { op: "expr", source: null }))}
            >
              外す
            </Button>
          )}
          <Button
            size="sm"
            onClick={() =>
              (setOpen(false), void propOp("エクスプレッション", layer.id, prop.path, { op: "expr", source: text.trim() ? text : null }))
            }
          >
            適用
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
