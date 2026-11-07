// 小さな部品: 数値欄（ラベルを左右にドラッグして変える）・アイコンボタン・ポップオーバーのメニュー。

import { type ReactNode, useEffect, useRef, useState } from "react";
import { Icon } from "./icons";

export function IconButton(props: {
  icon: string;
  title: string;
  on?: boolean;
  disabled?: boolean;
  size?: number;
  className?: string;
  onClick?: (e: React.MouseEvent) => void;
}) {
  return (
    <button
      type="button"
      className={`icon-btn ${props.on ? "on" : ""} ${props.className ?? ""}`}
      title={props.title}
      aria-label={props.title}
      aria-pressed={props.on}
      disabled={props.disabled}
      onClick={props.onClick}
    >
      <Icon name={props.icon} size={props.size ?? 16} />
    </button>
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
  label?: string;
  disabled?: boolean;
  width?: number;
}) {
  const { value, step = 1, digits = 1, unit = "" } = props;
  const [editing, setEditing] = useState<string | null>(null);
  const drag = useRef<{ x: number; v: number; moved: boolean } | null>(null);
  const clamp = (v: number) => Math.min(props.max ?? Infinity, Math.max(props.min ?? -Infinity, v));

  const onPointerDown = (e: React.PointerEvent) => {
    if (props.disabled || editing !== null) return;
    (e.target as Element).setPointerCapture(e.pointerId);
    drag.current = { x: e.clientX, v: value, moved: false };
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    if (!d.moved && Math.abs(dx) < 3) return;
    d.moved = true;
    const mul = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
    props.onChange(clamp(d.v + dx * step * mul), true);
  };
  const onPointerUp = (e: React.PointerEvent) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (d.moved) {
      const mul = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
      props.onChange(clamp(d.v + (e.clientX - d.x) * step * mul), false);
    } else setEditing(fmt(value, digits));
  };
  const commit = () => {
    if (editing === null) return;
    const v = Number(editing);
    setEditing(null);
    if (Number.isFinite(v) && v !== value) props.onChange(clamp(v), false);
  };

  return (
    <span className={`num ${props.disabled ? "disabled" : ""}`} style={props.width ? { width: props.width } : undefined}>
      {props.label && <span className="num-label">{props.label}</span>}
      {editing !== null ? (
        <input
          autoFocus
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
      ) : (
        <span className="num-value" onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp}>
          {fmt(value, digits)}
          {unit && <span className="num-unit">{unit}</span>}
        </span>
      )}
    </span>
  );
}

/** 文字の入力欄。Enter かフォーカスが外れたときに確定する。 */
export function TextField(props: { value: string; onChange: (v: string) => void; placeholder?: string; multiline?: boolean }) {
  const [v, setV] = useState(props.value);
  useEffect(() => setV(props.value), [props.value]);
  const commit = () => v !== props.value && props.onChange(v);
  const common = {
    className: "text-field",
    value: v,
    placeholder: props.placeholder,
    onBlur: commit,
  };
  return props.multiline ? (
    <textarea {...common} rows={3} onChange={(e) => setV(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
  ) : (
    <input
      {...common}
      onChange={(e) => setV(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        e.stopPropagation();
      }}
    />
  );
}

/** ボタンの近くに開く一覧。外を押すか項目を選ぶと閉じる。 */
export function Popover(props: {
  trigger: (open: boolean, toggle: () => void) => ReactNode;
  children: (close: () => void) => ReactNode;
  placement?: "above" | "below";
  align?: "start" | "center" | "end";
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("keydown", key);
    };
  }, [open]);
  return (
    <span className="popover-anchor" ref={ref}>
      {props.trigger(open, () => setOpen((o) => !o))}
      {open && (
        <div className={`popover ${props.placement ?? "below"} align-${props.align ?? "start"}`}>{props.children(() => setOpen(false))}</div>
      )}
    </span>
  );
}

export function MenuItem(props: { icon?: string; label: string; hint?: string; onClick: () => void; disabled?: boolean; checked?: boolean }) {
  return (
    <button type="button" className="menu-item" disabled={props.disabled} onClick={props.onClick}>
      <span className="menu-icon">{props.checked !== undefined ? (props.checked ? "✓" : "") : props.icon && <Icon name={props.icon} />}</span>
      <span className="menu-label">{props.label}</span>
      {props.hint && <span className="menu-hint">{props.hint}</span>}
    </button>
  );
}

/** 折りたためる区画（インスペクタ）。 */
export function Section(props: { title: string; children: ReactNode; defaultOpen?: boolean; actions?: ReactNode }) {
  const [open, setOpen] = useState(props.defaultOpen ?? true);
  return (
    <section className="section">
      <header className="section-head" onClick={() => setOpen((o) => !o)}>
        <Icon name="twirl" size={10} className={`twirl ${open ? "open" : ""}`} />
        <span>{props.title}</span>
        <span className="section-actions" onClick={(e) => e.stopPropagation()}>
          {props.actions}
        </span>
      </header>
      {open && <div className="section-body">{props.children}</div>}
    </section>
  );
}

export function Row(props: { label: string; children: ReactNode; title?: string }) {
  return (
    <div className="row" title={props.title}>
      <span className="row-label">{props.label}</span>
      <span className="row-value">{props.children}</span>
    </div>
  );
}
