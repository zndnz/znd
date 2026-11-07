// 画面で使う小さな部品。見た目は shadcn/ui（プリセット beEgoEQi）の部品に任せ、ここでは組み合わせ方だけを決める。

import { type ReactNode, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { Icon } from "./icons";

/** アイコンだけのボタン。ポインタを載せると名前が出る。`on` なら押された見た目にする。 */
export function IconButton(props: {
  icon: string;
  title: string;
  on?: boolean;
  disabled?: boolean;
  size?: "icon-xs" | "icon-sm" | "icon" | "icon-lg";
  iconSize?: number;
  filled?: boolean;
  className?: string;
  onClick?: (e: React.MouseEvent) => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            variant="ghost"
            size={props.size ?? "icon-sm"}
            aria-label={props.title}
            aria-pressed={props.on}
            disabled={props.disabled}
            onClick={props.onClick}
            className={cn("text-muted-foreground hover:text-foreground aria-pressed:bg-muted aria-pressed:text-foreground", props.className)}
          />
        }
      >
        <Icon name={props.icon} size={props.iconSize ?? 16} filled={props.filled} />
      </TooltipTrigger>
      <TooltipContent>{props.title}</TooltipContent>
    </Tooltip>
  );
}

const fmt = (v: number, digits: number) => {
  const s = v.toFixed(digits);
  return digits > 0 ? s.replace(/\.?0+$/, "") || "0" : s;
};

/**
 * 数値欄。押したまま左右にドラッグすると値が変わり（Shift で 10 倍、Alt で 1/10）、クリックすると入力できる。
 * ドラッグ中の変更は `onChange(v, true)`（途中）、確定は `onChange(v, false)`。
 */
export function NumberField(props: {
  value: number;
  onChange: (v: number, dragging: boolean) => void;
  step?: number;
  min?: number;
  max?: number;
  digits?: number;
  unit?: string;
  disabled?: boolean;
  className?: string;
}) {
  const { value, step = 1, digits = 1, unit = "" } = props;
  const [editing, setEditing] = useState<string | null>(null);
  const drag = useRef<{ x: number; v: number; moved: boolean } | null>(null);
  const clamp = (v: number) => Math.min(props.max ?? Infinity, Math.max(props.min ?? -Infinity, v));
  const at = (e: React.PointerEvent, d: { x: number; v: number }) => {
    const mul = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
    return clamp(d.v + (e.clientX - d.x) * step * mul);
  };
  const commit = () => {
    if (editing === null) return;
    const v = Number(editing);
    setEditing(null);
    if (Number.isFinite(v) && v !== value) props.onChange(clamp(v), false);
  };
  const box = "h-8 min-w-0 flex-1 rounded-3xl bg-input/50 px-3 text-sm tabular-nums";

  if (editing !== null) {
    return (
      <Input
        autoFocus
        className={cn(box, "h-8", props.className)}
        value={editing}
        onChange={(e) => setEditing(e.target.value)}
        onFocus={(e) => e.target.select()}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setEditing(null);
          e.stopPropagation();
        }}
      />
    );
  }
  return (
    <span
      className={cn(box, "inline-flex cursor-ew-resize items-center gap-0.5 select-none hover:bg-input/80", props.disabled && "pointer-events-none opacity-50", props.className)}
      onPointerDown={(e) => {
        if (props.disabled) return;
        (e.target as Element).setPointerCapture(e.pointerId);
        drag.current = { x: e.clientX, v: value, moved: false };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d || (!d.moved && Math.abs(e.clientX - d.x) < 3)) return;
        d.moved = true;
        props.onChange(at(e, d), true);
      }}
      onPointerUp={(e) => {
        const d = drag.current;
        drag.current = null;
        if (!d) return;
        if (d.moved) props.onChange(at(e, d), false);
        else setEditing(fmt(value, digits));
      }}
    >
      {fmt(value, digits)}
      {unit && <span className="text-muted-foreground">{unit}</span>}
    </span>
  );
}

/** 文字の入力欄。Enter かフォーカスが外れたときに確定する。 */
export function TextField(props: { value: string; onChange: (v: string) => void; placeholder?: string; multiline?: boolean; className?: string }) {
  const [v, setV] = useState(props.value);
  useEffect(() => setV(props.value), [props.value]);
  const commit = () => v !== props.value && props.onChange(v);
  if (props.multiline) {
    return (
      <Textarea
        className={cn("min-h-20 rounded-3xl", props.className)}
        value={v}
        placeholder={props.placeholder}
        onChange={(e) => setV(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.stopPropagation()}
      />
    );
  }
  return (
    <Input
      className={cn("h-8", props.className)}
      value={v}
      placeholder={props.placeholder}
      onChange={(e) => setV(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        e.stopPropagation();
      }}
    />
  );
}

/** 選択欄。`options` は [値, 表示名]。 */
export function Choice(props: { value: string; options: [string, string][]; onChange: (v: string) => void; className?: string }) {
  const items = props.options.map(([value, label]) => ({ value, label }));
  return (
    <Select items={items} value={props.value} onValueChange={(v) => v !== null && props.onChange(String(v))}>
      <SelectTrigger size="sm" className={cn("w-full min-w-0", props.className)}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent alignItemWithTrigger={false}>
        {items.map((i) => (
          <SelectItem key={i.value} value={i.value}>
            {i.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

/** 折りたためる区画（インスペクタ）。 */
export function Section(props: {
  title: string;
  children: ReactNode;
  defaultOpen?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  actions?: ReactNode;
}) {
  const controlled = props.open !== undefined ? { open: props.open, onOpenChange: props.onOpenChange } : { defaultOpen: props.defaultOpen ?? true };
  return (
    <Collapsible {...controlled} className="mx-2 border-t border-border">
      <div className="flex h-11 items-center gap-2 px-2">
        <CollapsibleTrigger className="group/trig flex flex-1 items-center gap-2 text-sm font-semibold outline-none">
          <Icon name="twirl" size={14} className="text-muted-foreground transition-transform group-data-panel-open/trig:rotate-90" />
          {props.title}
        </CollapsibleTrigger>
        {props.actions}
      </div>
      <CollapsibleContent className="flex flex-col gap-2 px-2 pb-4">{props.children}</CollapsibleContent>
    </Collapsible>
  );
}

export function Row(props: { label: string; children: ReactNode; title?: string }) {
  return (
    <div className="flex min-h-8 items-center gap-3" title={props.title}>
      <span className="w-24 shrink-0 text-sm text-muted-foreground">{props.label}</span>
      <span className="flex min-w-0 flex-1 items-center gap-1.5">{props.children}</span>
    </div>
  );
}
