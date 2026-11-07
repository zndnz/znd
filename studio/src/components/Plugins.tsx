// プラグイン: コマンドの実行（メニュー・ツールバー・レイヤーの右クリック・ショートカット）、スクリプトで組み立てる
// パネル、出力フィルター、管理（安全性スキャンの結果・自己責任での許可・ネイティブのプラグインの設定・無効化）。
// スクリプトの実行と、プロジェクト・ファイルに関わる操作は Rust（crates/znd-studio/src/plugins.rs）が行う。

import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { PanelItem, PluginCommand, PluginCtx, PluginOutcome, PluginReport, Snapshot } from "../model";
import { fps } from "../model";
import { adopt, currentComp, get, set, setFrame, totalFrames, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ContextMenuItem, ContextMenuSub, ContextMenuSubContent, ContextMenuSubTrigger } from "@/components/ui/context-menu";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { Choice, IconButton, NumberField, TextField } from "./controls";
import { openComp } from "./ProjectList";

// ---------- 状態と実行 ----------

/** プラグインの状態を取り直す（定義が変わったら、エフェクトの一覧なども取り直させる）。 */
export async function refreshPlugins(definitionsChanged = false) {
  try {
    const plugins = await api.pluginState();
    set((s) => ({ plugins, pluginGen: definitionsChanged || !s.plugins ? s.pluginGen + 1 : s.pluginGen }));
  } catch (e) {
    set({ status: `プラグインの状態を読めません: ${e}` });
  }
}

export function pluginCtx(): PluginCtx {
  const s = get();
  return { comp: s.compId, layer: s.selected, frame: s.frame };
}

const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);

/** 実行の結果を画面に反映する（新しいプロジェクト・選択や再生ヘッドなどの操作・お知らせ）。 */
export function applyOutcome(o: PluginOutcome) {
  if (o.snapshot) adopt(o.snapshot);
  for (const a of o.actions) {
    switch (a.type) {
      case "select": {
        const id = str(a.layer) || null;
        set({ selected: id, selectedFootage: null, selectedKeys: null });
        break;
      }
      case "frame":
        setFrame(num(a.frame) ?? 0);
        break;
      case "time": {
        const c = currentComp();
        if (c) setFrame(Math.round((num(a.seconds) ?? 0) * fps(c.frame_rate)));
        break;
      }
      case "work_area": {
        const [s, e] = [num(a.start), num(a.end)];
        const total = totalFrames();
        if (s === null || e === null) set({ workArea: null });
        else {
          const [a0, b0] = [Math.max(0, Math.min(total, s)), Math.max(0, Math.min(total, e))];
          set({ workArea: b0 > a0 ? [a0, b0] : null });
        }
        break;
      }
      case "status":
        set({ status: str(a.text) });
        break;
      case "alert":
        set({ alert: str(a.text) });
        break;
      case "copy_text":
        void navigator.clipboard.writeText(str(a.text)).catch(() => set({ status: "クリップボードに入れられませんでした" }));
        break;
      case "comp": {
        const id = str(a.comp);
        if (get().project?.compositions.some((c) => c.id === id)) openComp(id);
        break;
      }
      case "panel":
        setPanelOpen(str(a.key), a.open !== false);
        break;
    }
  }
  if (o.status) set({ status: o.status });
  void refreshPlugins(o.changed);
}

export async function runPluginCommand(key: string) {
  try {
    applyOutcome(await api.pluginCommand(key, pluginCtx()));
  } catch (e) {
    set({ status: String(e) });
  }
}

export function setPanelOpen(key: string, open: boolean) {
  set((s) => ({ openPanels: open ? [...new Set([...s.openPanels, key])] : s.openPanels.filter((k) => k !== key) }));
}

/** 管理の操作の結果（新しい状態とメッセージ）を反映する。 */
async function managed(f: () => Promise<{ snapshot: Snapshot; status: string } | Snapshot | string | null | void>) {
  try {
    const r = await f();
    if (r && typeof r === "object" && "snapshot" in r) {
      adopt(r.snapshot);
      set({ status: r.status });
    } else if (r && typeof r === "object") adopt(r);
    else if (typeof r === "string") set({ status: r });
  } catch (e) {
    set({ status: String(e) });
  }
  await refreshPlugins(true);
}

/** メニューの項目（plugin_cmd: など）を処理する。処理したら true。 */
export function onPluginMenu(id: string): boolean {
  if (id.startsWith("plugin_cmd:")) return void runPluginCommand(id.slice(11)), true;
  if (id.startsWith("plugin_panel:")) {
    const key = id.slice(13);
    setPanelOpen(key, !get().openPanels.includes(key));
    return true;
  }
  if (id.startsWith("plugin_filter:")) {
    const key = id.slice(14);
    const on = !get().plugins?.filters.find((f) => f.key === key)?.enabled;
    void managed(() => api.pluginSetFilter(key, on));
    return true;
  }
  if (id === "plugins_manage") return set({ dialog: "plugins" }), void refreshPlugins(), true;
  if (id === "plugins_log") return set({ dialog: "pluginLog" }), void refreshPlugins(), true;
  return false;
}

// ---------- ショートカット ----------

/** "Cmd+Shift+K" のような表記が、押されたキーと合うか。 */
function matches(shortcut: string, e: KeyboardEvent): boolean {
  let cmd = false,
    ctrl = false,
    alt = false,
    shift = false,
    key = "";
  for (const part of shortcut.split("+").map((p) => p.trim().toLowerCase())) {
    if (["cmd", "command", "mod"].includes(part)) cmd = true;
    else if (["ctrl", "control"].includes(part)) ctrl = true;
    else if (["alt", "option", "opt"].includes(part)) alt = true;
    else if (part === "shift") shift = true;
    else key = part;
  }
  const pressed = e.code.startsWith("Key") ? e.code.slice(3).toLowerCase() : e.code.startsWith("Digit") ? e.code.slice(5) : e.key.toLowerCase();
  return pressed === key && e.metaKey === cmd && e.ctrlKey === ctrl && e.altKey === alt && e.shiftKey === shift;
}

/**
 * プラグインのショートカット。「プラグイン」メニューに載るものはメニューが受けるので、ここでは
 * ツールバー・レイヤーの右クリックにだけ出るコマンドのショートカットを受ける。受けたら true。
 */
export function onPluginKey(e: KeyboardEvent): boolean {
  const cmds = get().plugins?.commands ?? [];
  const hit = cmds.find((c) => c.shortcut && !c.menu.includes("plugins") && matches(c.shortcut, e));
  if (!hit) return false;
  e.preventDefault();
  void runPluginCommand(hit.key);
  return true;
}

// ---------- ツールバー・レイヤーの右クリック ----------

export function PluginToolbar() {
  const cmds = useUi((s) => s.plugins?.commands.filter((c) => c.menu.includes("toolbar")) ?? []);
  if (cmds.length === 0) return null;
  return (
    <span className="flex items-center gap-1">
      {cmds.map((c) => (
        <Button
          key={c.key}
          variant="ghost"
          size="sm"
          className="h-7 rounded-3xl px-2.5 text-xs"
          title={`${c.description || c.label}（プラグイン: ${c.plugin}）`}
          onClick={() => void runPluginCommand(c.key)}
        >
          {c.label}
        </Button>
      ))}
    </span>
  );
}

export function PluginLayerMenu({ layer }: { layer: string }) {
  const cmds: PluginCommand[] = useUi((s) => s.plugins?.commands.filter((c) => c.menu.includes("layer")) ?? []);
  if (cmds.length === 0) return null;
  return (
    <ContextMenuSub>
      <ContextMenuSubTrigger>プラグイン</ContextMenuSubTrigger>
      <ContextMenuSubContent className="w-56">
        {cmds.map((c) => (
          <ContextMenuItem
            key={c.key}
            title={c.description}
            onClick={() => {
              set({ selected: layer, selectedFootage: null });
              void runPluginCommand(c.key);
            }}
          >
            {c.label}
          </ContextMenuItem>
        ))}
      </ContextMenuSubContent>
    </ContextMenuSub>
  );
}

// ---------- スクリプトのパネル ----------

export function PluginPanels() {
  const open = useUi((s) => s.openPanels);
  const panels = useUi((s) => s.plugins?.panels ?? []);
  return (
    <>
      {open.map((key, i) => {
        const p = panels.find((x) => x.key === key);
        return p ? <PanelWindow key={key} panelKey={key} title={p.title} plugin={p.plugin} width={p.width} index={i} /> : null;
      })}
    </>
  );
}

/** パネル 1 つ。部品はスクリプトの `panel(state)` が返し、操作は `onEvent(id, value, state)` へ送る。 */
function PanelWindow(props: { panelKey: string; title: string; plugin: string; width: number; index: number }) {
  const { panelKey } = props;
  const revision = useUi((s) => s.snap?.revision ?? 0);
  const selected = useUi((s) => s.selected);
  const compId = useUi((s) => s.compId);
  const [items, setItems] = useState<PanelItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const [pos, setPos] = useState({ x: 120 + props.index * 28, y: 90 + props.index * 28 });

  // プロジェクト・選択・コンポが変わったら作り直す
  useEffect(() => {
    let alive = true;
    void api
      .pluginPanel(panelKey, pluginCtx())
      .then((r) => {
        if (!alive) return;
        setItems(r.items);
        setError(r.error);
      })
      .catch((e) => alive && setError(String(e)));
    return () => {
      alive = false;
    };
  }, [panelKey, revision, selected, compId, tick]);

  const send = (id: string, value: unknown) =>
    void api
      .pluginPanelEvent(panelKey, id, value, pluginCtx())
      .then((o) => {
        applyOutcome(o);
        setTick((t) => t + 1);
      })
      .catch((e) => setError(String(e)));

  const drag = (e: React.PointerEvent) => {
    const start = { x: e.clientX, y: e.clientY, pos };
    const move = (ev: PointerEvent) => setPos({ x: start.pos.x + ev.clientX - start.x, y: Math.max(0, start.pos.y + ev.clientY - start.y) });
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div
      className="fixed z-40 flex max-h-[70vh] flex-col overflow-hidden rounded-3xl bg-popover text-sm text-popover-foreground shadow-xl ring-1 ring-foreground/10"
      style={{ left: pos.x, top: pos.y, width: Math.max(220, props.width || 280) }}
    >
      <div className="flex h-10 cursor-grab items-center gap-2 pr-1.5 pl-4" onPointerDown={drag}>
        <span className="flex-1 truncate font-semibold">{props.title}</span>
        <IconButton icon="close" title="閉じる" size="icon-xs" onClick={() => setPanelOpen(panelKey, false)} />
      </div>
      <div className="flex flex-col gap-2 overflow-y-auto px-4 pb-3">
        {error && <p className="text-xs text-destructive">{error}</p>}
        {items.map((it, i) => (
          <PanelPart key={`${it.id ?? ""}-${i}`} item={it} send={send} />
        ))}
        <p className="pt-1 text-[11px] text-muted-foreground/60">プラグイン: {props.plugin}</p>
      </div>
    </div>
  );
}

const toHex = (c: number[]) =>
  "#" + c.slice(0, 3).map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, "0")).join("");
const fromHex = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);

function PanelPart({ item, send }: { item: PanelItem; send: (id: string, v: unknown) => void }) {
  const id = item.id ?? "";
  const label = item.label ?? "";
  const labeled = (control: React.ReactNode) => (
    <div className="flex min-h-8 items-center gap-3">
      {label && <span className="w-24 shrink-0 text-muted-foreground">{label}</span>}
      <span className="flex min-w-0 flex-1 items-center gap-1.5">{control}</span>
    </div>
  );
  switch (item.type) {
    case "heading":
      return <div className="pt-1 font-semibold">{item.text}</div>;
    case "label":
      return <div>{item.text}</div>;
    case "hint":
      return <div className="text-xs text-muted-foreground">{item.text}</div>;
    case "separator":
      return <div className="my-1 h-px bg-border" />;
    case "button":
      return (
        <Button variant="secondary" size="sm" className="self-start" onClick={() => send(id, null)}>
          {label}
        </Button>
      );
    case "slider": {
      const lo = Math.min(item.min ?? 0, item.max ?? 1);
      const hi = Math.max(item.min ?? 0, item.max ?? 1);
      const v = typeof item.value === "number" ? item.value : 0;
      return labeled(
        <>
          <Slider
            className="flex-1"
            min={lo}
            max={hi}
            step={item.step ?? (hi - lo) / 100}
            value={v}
            onValueChange={(x) => send(id, Array.isArray(x) ? x[0] : x)}
          />
          <span className="w-12 text-right text-xs tabular-nums text-muted-foreground">{Number(v.toFixed(3))}</span>
        </>,
      );
    }
    case "number":
      return labeled(
        <NumberField value={typeof item.value === "number" ? item.value : 0} step={item.speed ?? 0.5} digits={3} onChange={(v, d) => !d && send(id, v)} />,
      );
    case "checkbox":
      return (
        <label className="flex min-h-8 items-center gap-2">
          <Checkbox checked={item.value === true} onCheckedChange={(on) => send(id, on)} />
          {label}
        </label>
      );
    case "text":
      return labeled(<TextField value={typeof item.value === "string" ? item.value : ""} onChange={(v) => send(id, v)} />);
    case "choice": {
      const options = item.options ?? [];
      const cur = Math.max(0, Math.floor(typeof item.value === "number" ? item.value : 0));
      return labeled(<Choice value={String(cur)} options={options.map((o, i): [string, string] => [String(i), o])} onChange={(v) => send(id, Number(v))} />);
    }
    case "color": {
      const c = Array.isArray(item.value) ? (item.value as number[]) : [1, 1, 1, 1];
      return labeled(
        <input type="color" className="h-8 w-12 rounded-3xl" value={toHex(c)} onChange={(e) => send(id, [...fromHex(e.target.value), c[3] ?? 1])} />,
      );
    }
    case "row":
      return (
        <div className="flex flex-wrap items-center gap-2">
          {(item.items ?? []).map((sub, i) => (
            <PanelPart key={`${sub.id ?? ""}-${i}`} item={sub} send={send} />
          ))}
        </div>
      );
    default:
      return <div className="text-xs text-muted-foreground">（未対応の部品: {item.type}）</div>;
  }
}

// ---------- お知らせ・異常終了の確認 ----------

export function PluginNotices() {
  const alert = useUi((s) => s.alert);
  const crash = useUi((s) => s.plugins?.crash ?? null);
  return (
    <>
      <Dialog open={alert !== null} onOpenChange={(o) => !o && set({ alert: null })}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>プラグインからのお知らせ</DialogTitle>
          </DialogHeader>
          <p className="whitespace-pre-wrap">{alert}</p>
          <DialogFooter>
            <Button onClick={() => set({ alert: null })}>閉じる</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={crash !== null} onOpenChange={(o) => !o && void managed(() => api.pluginCrashAnswer(false))}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>プラグインによる異常終了</DialogTitle>
            <DialogDescription>前回、プラグイン「{crash?.[1]}」の処理中にアプリが異常終了しました。</DialogDescription>
          </DialogHeader>
          <p className="text-muted-foreground">
            無効にすると、このエフェクトは元の画像をそのまま通します（プロジェクトの設定は残ります）。プラグインの管理からいつでも戻せます。
          </p>
          <DialogFooter>
            <Button variant="ghost" onClick={() => void managed(() => api.pluginCrashAnswer(false))}>
              そのまま使う
            </Button>
            <Button variant="destructive" onClick={() => void managed(() => api.pluginCrashAnswer(true))}>
              このプラグインを無効にする
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

// ---------- 管理 ----------

const RISK: Record<PluginReport["risk"], string> = {
  safe: "bg-emerald-500",
  low: "bg-lime-400",
  medium: "bg-amber-400",
  high: "bg-orange-500",
  critical: "bg-red-500",
};

function Risk({ report }: { report: PluginReport }) {
  return <span className={cn("rounded-full px-2 py-0.5 text-[11px] font-semibold text-black", RISK[report.risk])}>危険度: {report.level}</span>;
}

function ReportView({ report, compact }: { report: PluginReport; compact?: boolean }) {
  if (report.findings.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        {report.files_scanned} 個のファイルを調べ、通信・個人情報の読み取り・プログラムの実行などの痕跡は見つかりませんでした。
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1 text-xs">
      <p className="font-medium">{report.summary}</p>
      {report.findings.slice(0, compact ? 5 : 64).map((f, i) => (
        <div key={i}>
          <span className="text-muted-foreground">
            [{f.category}] +{f.score}
          </span>{" "}
          {f.title}
          {f.evidence.length > 0 && <div className="pl-4 font-mono text-[11px] break-all text-muted-foreground">根拠: {f.evidence.join(", ")}</div>}
        </div>
      ))}
      {!compact && report.endpoints.length > 0 && <p className="text-muted-foreground">見つかった通信先: {report.endpoints.join(", ")}</p>}
      {!compact && <p className="font-mono text-[10px] break-all text-muted-foreground/60">SHA-256: {report.hash}</p>}
    </div>
  );
}

function Card({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("flex flex-col gap-1.5 rounded-3xl bg-muted/50 p-3", className)}>{children}</div>;
}

const or = (s: string) => s || "記載なし";

export function PluginManager() {
  const open = useUi((s) => s.dialog === "plugins");
  const st = useUi((s) => s.plugins);
  const [detail, setDetail] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ path: string; name: string; report: PluginReport } | null>(null);
  const close = () => set({ dialog: null });
  return (
    <>
      <Dialog open={open} onOpenChange={(o) => !o && close()}>
        <DialogContent className="flex max-h-[86vh] flex-col sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>プラグイン</DialogTitle>
          </DialogHeader>
          {st && (
            <div className="-mx-6 flex min-h-0 flex-col gap-4 overflow-y-auto px-6">
              <div className="rounded-3xl bg-amber-500/10 p-3 text-xs">
                <p className="font-semibold text-amber-400">⚠ プラグインの導入・使用は自己責任です</p>
                <p className="mt-1 whitespace-pre-wrap">{st.disclaimer}</p>
                <p className="mt-1 text-muted-foreground">
                  エフェクトのプラグイン（WebAssembly）はサンドボックスの中で動き、ファイル・ネットワーク・別のプログラムには触れられません。
                  スクリプト（コマンド・パネル）も、ファイルは利用者がダイアログで選んだものだけを扱えます。
                  ネイティブのプラグイン（.dylib / .so / .dll / OpenFX）はこのアプリと同じ権限で動きます。
                </p>
              </div>

              <div className="flex flex-wrap gap-2">
                <Button variant="secondary" size="sm" title="エフェクトの WebAssembly（.wasm）を選びます（許可していればネイティブのライブラリも）。この起動の間だけ使えます" onClick={() => void managed(() => api.pluginLoad(false))}>
                  プラグインを読み込む…
                </Button>
                <Button variant="secondary" size="sm" title="plugin.json のあるパッケージ、または OpenFX の .ofx.bundle" onClick={() => void managed(() => api.pluginLoad(true))}>
                  フォルダを読み込む…
                </Button>
                <Button variant="secondary" size="sm" onClick={() => void managed(api.pluginRescan)}>
                  再走査
                </Button>
                {st.plugin_dir && (
                  <Button variant="secondary" size="sm" title={st.plugin_dir} onClick={() => void managed(api.pluginOpenDir)}>
                    プラグインフォルダを開く
                  </Button>
                )}
              </div>

              <label className="flex items-start gap-3 text-xs">
                <Switch checked={st.native_allowed} onCheckedChange={(on) => void managed(() => api.pluginSetNative(on))} />
                <span>
                  サンドボックスの外で動くネイティブのプラグイン（.dylib / .so / .dll / OpenFX）も読み込む
                  <span className="block text-muted-foreground">
                    アプリと同じ権限で動き、ファイルの読み書きや通信など何でもできてしまいます。入手元を信頼できるものだけにしてください。変更は次の起動（または再走査）から効きます。
                  </span>
                  {st.native_allowed && <span className="block text-amber-400">ネイティブのプラグインを許可しています。信頼できるものだけを入れてください。</span>}
                </span>
              </label>

              {(st.disabled.length > 0 || st.faults.length > 0) && (
                <section className="flex flex-col gap-2">
                  <h3 className="font-semibold text-amber-400">使っていないプラグイン</h3>
                  {st.disabled.map((kind) => (
                    <Card key={kind} className="flex-row items-center">
                      <span className="flex-1">無効: {kind}</span>
                      <Button size="sm" variant="ghost" onClick={() => void managed(() => api.pluginSetDisabled(kind, false))}>
                        有効に戻す
                      </Button>
                    </Card>
                  ))}
                  {st.faults.map((f) => (
                    <Card key={f.kind}>
                      <div className="flex items-center gap-2">
                        <span className="flex-1">停止中: {f.kind}</span>
                        {f.quarantined && (
                          <Button size="sm" variant="ghost" onClick={() => void managed(() => api.pluginRelease(f.kind))}>
                            もう一度使う
                          </Button>
                        )}
                        {!f.disabled && (
                          <Button size="sm" variant="ghost" onClick={() => void managed(() => api.pluginSetDisabled(f.kind, true))}>
                            無効にする
                          </Button>
                        )}
                      </div>
                      <p className="text-xs text-muted-foreground">{f.why}</p>
                    </Card>
                  ))}
                </section>
              )}

              {st.blocked.length > 0 && (
                <section className="flex flex-col gap-2">
                  <h3 className="font-semibold text-destructive">読み込みを止めているプラグイン（{st.blocked.length}）</h3>
                  <p className="text-xs text-muted-foreground">安全性スキャンで危険度が「高」以上と判定されたため、読み込んでいません。</p>
                  {st.blocked.map((b) => (
                    <Card key={b.path}>
                      <div className="flex items-center gap-2">
                        <Risk report={b.report} />
                        <span className="font-medium">{b.name}</span>
                      </div>
                      <p className="text-xs break-all text-muted-foreground">{b.path}</p>
                      <ReportView report={b.report} compact />
                      <div className="flex items-center gap-2">
                        <Button size="sm" variant="destructive" onClick={() => setConfirm(b)}>
                          自己責任で許可して読み込む…
                        </Button>
                        <span className="text-xs text-muted-foreground">許可は中身のハッシュに結び付けて記録され、ファイルが変わると再び止まります</span>
                      </div>
                    </Card>
                  ))}
                </section>
              )}

              {st.packages.length > 0 && (
                <section className="flex flex-col gap-2">
                  <h3 className="font-semibold">パッケージ（コマンド・パネル・出力フィルター・エクスプレッション・書き出し形式）</h3>
                  {st.packages.map((p) => (
                    <Card key={p.dir}>
                      <div className="flex items-center gap-2">
                        <Risk report={p.report} />
                        <span className="font-medium">{p.name}</span>
                        <span className="text-xs text-muted-foreground">
                          {p.id} {p.version}
                        </span>
                      </div>
                      {p.description && <p>{p.description}</p>}
                      <p className="text-xs">
                        提供元: {or(p.vendor)}　ライセンス: {or(p.license)}　宣言された権限: {p.permissions.length ? p.permissions.join(", ") : "なし"}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        コマンド {p.commands}・パネル {p.panels}・出力フィルター {p.filters}・エクスプレッション用ライブラリ {p.expressions}・書き出し形式 {p.exporters}・エフェクト {p.effects}
                      </p>
                      {p.failed && <p className="text-xs text-destructive">読み込めません: {p.failed}</p>}
                      {p.warnings.map((w, i) => (
                        <p key={i} className="text-xs text-amber-400">
                          {w}
                        </p>
                      ))}
                      <p className="text-xs break-all text-muted-foreground/70">{p.dir}</p>
                      <div className="flex gap-2">
                        <Button size="sm" variant="ghost" className="self-start" onClick={() => setDetail(detail === p.dir ? null : p.dir)}>
                          {detail === p.dir ? "スキャン結果を閉じる" : "スキャン結果を見る"}
                        </Button>
                        {detail === p.dir && p.report.blocks && (
                          <Button size="sm" variant="ghost" onClick={() => void managed(() => api.pluginRevoke(p.dir))}>
                            許可を取り消す（次の起動から止める）
                          </Button>
                        )}
                      </div>
                      {detail === p.dir && <ReportView report={p.report} />}
                    </Card>
                  ))}
                </section>
              )}

              <section className="flex flex-col gap-2">
                <h3 className="font-semibold">エフェクト（znd ネイティブ / WASM / OpenFX）</h3>
                {st.effects.length === 0 && (
                  <p className="text-xs text-muted-foreground">読み込まれているエフェクトはありません。サンプルは examples/plugins、作り方は docs/06-plugins.md にあります。</p>
                )}
                {st.effects.map((i) => (
                  <Card key={i.kind}>
                    <div className="flex items-center gap-2">
                      {i.report && <Risk report={i.report} />}
                      <span className="font-medium">{i.label}</span>
                      <span className="text-xs text-muted-foreground">
                        [{i.api} / {i.category}]
                      </span>
                    </div>
                    <p className="text-xs">
                      提供元: {or(i.vendor)}　版: {or(i.version)}　ライセンス: {or(i.license)}
                    </p>
                    <p className="text-xs break-all text-muted-foreground">
                      {i.kind}（パラメータ {i.params} 個）　{i.path}
                    </p>
                    {i.report && i.report.findings.length > 0 && (
                      <>
                        <Button size="sm" variant="ghost" className="self-start" onClick={() => setDetail(detail === i.path ? null : i.path)}>
                          {detail === i.path ? "スキャン結果を閉じる" : "スキャン結果を見る"}
                        </Button>
                        {detail === i.path && <ReportView report={i.report} />}
                      </>
                    )}
                  </Card>
                ))}
              </section>

              {st.missing.length > 0 && (
                <section className="flex flex-col gap-1 text-xs">
                  <h3 className="text-sm font-semibold text-destructive">このプロジェクトが使っているのに、入っていないプラグイン</h3>
                  {st.missing.map(([kind, layer], i) => (
                    <p key={i}>
                      {layer}: {kind}（このエフェクトは無視されます）
                    </p>
                  ))}
                </section>
              )}
              {st.errors.length > 0 && (
                <section className="flex flex-col gap-1 text-xs">
                  <h3 className="text-sm font-semibold text-destructive">読み込めなかったファイル</h3>
                  {st.errors.map(([path, e], i) => (
                    <p key={i} className="break-all">
                      {path}: {e}
                    </p>
                  ))}
                </section>
              )}
              {st.ofx_messages.length > 0 && (
                <section className="flex flex-col gap-1 text-xs">
                  <h3 className="text-sm font-semibold">OpenFX プラグインからのメッセージ</h3>
                  {st.ofx_messages.slice(-8).reverse().map((m, i) => (
                    <p key={i} className="text-muted-foreground">
                      {m}
                    </p>
                  ))}
                </section>
              )}
            </div>
          )}
        </DialogContent>
      </Dialog>

      <Dialog open={confirm !== null} onOpenChange={(o) => !o && setConfirm(null)}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>危険度の高いプラグインを許可しますか？</DialogTitle>
          </DialogHeader>
          {confirm && (
            <div className="flex flex-col gap-2">
              <div className="flex items-center gap-2">
                <Risk report={confirm.report} />
                <span className="font-medium">{confirm.name}</span>
              </div>
              <p>{confirm.report.summary}</p>
              {confirm.report.findings.slice(0, 4).map((f, i) => (
                <p key={i}>・{f.title}</p>
              ))}
              {confirm.report.endpoints.length > 0 && <p>通信先の候補: {confirm.report.endpoints.slice(0, 4).join(", ")}</p>}
              <p className="text-destructive">
                許可すると、このプラグインはこのアプリと同じ権限で動きます。情報の持ち出しや、外部からのプログラムの取得・実行が行われる恐れがあります。
              </p>
              <p className="text-xs whitespace-pre-wrap text-muted-foreground">{st?.disclaimer}</p>
            </div>
          )}
          <DialogFooter>
            <Button variant="ghost" onClick={() => setConfirm(null)}>
              やめる
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                const path = confirm!.path;
                setConfirm(null);
                void managed(() => api.pluginAllow(path));
              }}
            >
              内容を理解し、自己責任で許可する
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

export function PluginLog() {
  const open = useUi((s) => s.dialog === "pluginLog");
  const logs = useUi((s) => s.plugins?.logs ?? []);
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    end.current?.scrollIntoView();
  }, [logs.length, open]);
  return (
    <Dialog open={open} onOpenChange={(o) => !o && set({ dialog: null })}>
      <DialogContent className="flex max-h-[80vh] flex-col sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>プラグインのログ</DialogTitle>
        </DialogHeader>
        <div className="-mx-6 min-h-24 overflow-y-auto px-6 font-mono text-xs whitespace-pre-wrap select-text">
          {logs.length === 0 ? <p className="text-muted-foreground">ログはありません</p> : logs.map((l, i) => <div key={i}>{l}</div>)}
          <div ref={end} />
        </div>
        <DialogFooter>
          <Button variant="ghost" disabled={logs.length === 0} onClick={() => void managed(api.pluginClearLogs)}>
            ログを消す
          </Button>
          <Button onClick={() => set({ dialog: null })}>閉じる</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
