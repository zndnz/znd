// ウィンドウ（ダイアログ）: 書き出しとレンダーキュー、提出前チェック、取り消し履歴、版（スナップショット）、
// レビューコメント、使用 OSS とライセンス、自動保存からの復旧。重い処理は Rust がバックグラウンドで行い、
// 進み具合はタイトルバーに出す。

import { useEffect, useState } from "react";
import { api } from "../api";
import type { ExportFormat, ExportUi, ReviewComment } from "../model";
import { timecode, timeToFrame } from "../model";
import { adopt, currentComp, edit, get, reply, set, setFrame, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { Choice, IconButton, NumberField, Row, TextField } from "./controls";

const close = () => set({ dialog: null });
const useOpen = (name: string) => useUi((s) => s.dialog === name);

// ---------- バックグラウンド処理 ----------

/** 処理が動いている間、進み具合と結果を Rust に尋ねる。 */
export function useTasks() {
  const running = useUi((s) => s.task !== null);
  useEffect(() => {
    if (!running) return;
    const id = setInterval(async () => {
      try {
        const st = await api.taskStatus();
        set({ task: st.running });
        if (st.finished) {
          set({ status: st.finished.message });
          if (st.finished.check) set({ check: st.finished.check });
        }
      } catch {
        set({ task: null });
      }
    }, 300);
    return () => clearInterval(id);
  }, [running]);
}

async function startTask(f: () => Promise<void>, label: string) {
  try {
    await f();
    set({ task: [label, 0] });
  } catch (e) {
    set({ status: String(e) });
  }
}

export function TaskIndicator() {
  const task = useUi((s) => s.task);
  if (!task) return null;
  return (
    <span className="flex items-center gap-2 text-xs text-muted-foreground">
      {task[0]}
      <span className="h-1.5 w-24 overflow-hidden rounded-full bg-muted">
        <span className="block h-full bg-primary transition-[width]" style={{ width: `${Math.round(task[1] * 100)}%` }} />
      </span>
      {Math.round(task[1] * 100)}%
      <Button variant="ghost" size="sm" className="h-6 rounded-3xl px-2 text-xs" onClick={() => void api.taskCancel()}>
        中止
      </Button>
    </span>
  );
}

// ---------- 書き出し ----------

const DEFAULT_EXPORT: ExportUi = { format: 0, crf: 14, kbps: 12000, audio: true, pad: 0, transparent: false, range: null, frame: 0, values: {} };
let remembered: ExportUi = DEFAULT_EXPORT;

export function ExportDialog() {
  const open = useOpen("export");
  const comp = useUi(currentComp);
  const work = useUi((s) => s.workArea);
  const queue = useUi((s) => s.queue);
  const busy = useUi((s) => s.task !== null);
  const gen = useUi((s) => s.pluginGen);
  const [formats, setFormats] = useState<ExportFormat[]>([]);
  const [ui, setUi] = useState<ExportUi>(remembered);
  const [onlyWork, setOnlyWork] = useState(false);
  const [report, setReport] = useState(true);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [ack, setAck] = useState(false);
  useEffect(() => {
    if (!open || !comp) return;
    void api.exportFormats().then(setFormats);
    void api.exportWarnings(comp.id).then(setWarnings);
  }, [open, comp?.id, gen]);
  const patch = (p: Partial<ExportUi>) => setUi((u) => (remembered = { ...u, ...p }));
  const f = formats[ui.format];
  const settings = (): ExportUi => {
    const values = { ...Object.fromEntries((f?.params ?? []).map((p) => [p.id, p.default])), ...ui.values };
    return { ...ui, values, range: onlyWork && work ? work : null, frame: get().frame };
  };
  const allowed = warnings.length === 0 || ack;
  const pick = async () => (comp ? api.exportPick(comp.id, settings()) : null);

  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[86vh] flex-col sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>書き出し</DialogTitle>
          <DialogDescription>書き出し中も編集を続けられます。終わると通知します。</DialogDescription>
        </DialogHeader>
        <div className="-mx-6 flex min-h-0 flex-col gap-2 overflow-y-auto px-6">
          <Row label="形式">
            <Choice
              value={String(ui.format)}
              options={formats.map((x, i): [string, string] => [String(i), x.plugin ? `${x.label}（プラグイン）` : x.label])}
              onChange={(v) => patch({ format: Number(v), values: {} })}
            />
          </Row>
          {f?.description && <p className="text-xs text-muted-foreground">{f.description}</p>}
          {f?.params.map((p) => {
            const v = ui.values[p.id] ?? p.default;
            const setV = (x: string) => patch({ values: { ...ui.values, [p.id]: x } });
            return (
              <Row key={p.id} label={p.label}>
                {p.kind === "choice" ? (
                  <Choice value={v} options={p.choices.map(([label, value]): [string, string] => [value, label])} onChange={setV} />
                ) : p.kind === "bool" ? (
                  <Checkbox checked={v === "1"} onCheckedChange={(on) => setV(on ? "1" : "0")} />
                ) : p.kind === "text" || p.kind === "args" ? (
                  <TextField value={v} onChange={setV} />
                ) : (
                  <NumberField value={Number(v) || 0} digits={3} min={p.min ?? undefined} max={p.max ?? undefined} onChange={(x, d) => !d && setV(String(x))} />
                )}
              </Row>
            );
          })}
          {f?.single_frame && <p className="text-xs text-muted-foreground">今のフレームを 1 枚書き出します</p>}
          {f?.crf && (
            <Row label="画質 CRF" title="小さいほど高画質">
              <NumberField value={ui.crf} digits={0} min={0} max={40} onChange={(v) => patch({ crf: Math.round(v) })} />
            </Row>
          )}
          {f?.kbps && (
            <Row label="ビットレート">
              <NumberField value={ui.kbps} digits={0} step={100} min={500} max={200000} unit="kbps" onChange={(v) => patch({ kbps: Math.round(v) })} />
            </Row>
          )}
          <Row label="前後の黒味">
            <NumberField value={ui.pad} digits={1} step={0.1} min={0} max={30} unit="秒" onChange={(v) => patch({ pad: v })} />
          </Row>
          <label className="flex min-h-8 items-center gap-2">
            <Checkbox checked={ui.audio} onCheckedChange={(on) => patch({ audio: on })} />
            音声を含める
          </label>
          {f?.alpha && (
            <label className="flex min-h-8 items-center gap-2">
              <Checkbox checked={ui.transparent} onCheckedChange={(on) => patch({ transparent: on })} />
              背景を透過
            </label>
          )}
          <label className={cn("flex min-h-8 items-center gap-2", !work && "opacity-50")}>
            <Checkbox disabled={!work} checked={onlyWork && !!work} onCheckedChange={setOnlyWork} />
            ワークエリアだけ
          </label>
          <label className="flex min-h-8 items-center gap-2" title="解像度・フレーム数・ラウドネス・警告をテキストで残します">
            <Checkbox checked={report} onCheckedChange={setReport} />
            レポートを書き出す
          </label>
          {warnings.length > 0 && (
            <div className="flex flex-col gap-1 rounded-3xl bg-amber-500/10 p-3 text-xs">
              <p className="font-semibold text-amber-400">権利が確認できていない素材があります</p>
              {warnings.map((w, i) => (
                <p key={i}>・{w}</p>
              ))}
              <label className="mt-1 flex items-center gap-2">
                <Checkbox checked={ack} onCheckedChange={setAck} />
                確認中であることを承知のうえで書き出す
              </label>
            </div>
          )}
          {queue.length > 0 && (
            <div className="flex flex-col gap-1 pt-2">
              <p className="font-semibold">レンダーキュー</p>
              {queue.map((j, i) => (
                <div key={i} className="flex items-center gap-2 text-xs">
                  <span className="flex-1 truncate" title={j.path}>
                    {i + 1}. {get().project?.compositions.find((c) => c.id === j.comp)?.name ?? j.comp} → {j.path.split(/[\\/]/).pop()}
                  </span>
                  <IconButton icon="close" title="キューから外す" size="icon-xs" onClick={() => set((s) => ({ queue: s.queue.filter((_, k) => k !== i) }))} />
                </div>
              ))}
            </div>
          )}
        </div>
        <DialogFooter>
          {queue.length > 0 && (
            <Button
              variant="secondary"
              disabled={busy}
              onClick={() => {
                const jobs = get().queue;
                set({ queue: [] });
                void startTask(() => api.exportStart(jobs, report), `キューを書き出し中（${jobs.length} 件）`);
              }}
            >
              キューを実行
            </Button>
          )}
          <Button
            variant="secondary"
            disabled={!allowed || !comp}
            onClick={async () => {
              const path = await pick();
              if (path && comp) set((s) => ({ queue: [...s.queue, { comp: comp.id, path, settings: settings() }] }));
            }}
          >
            キューに追加…
          </Button>
          <Button
            disabled={!allowed || busy || !comp}
            onClick={async () => {
              const path = await pick();
              if (!path || !comp) return;
              void startTask(() => api.exportStart([{ comp: comp.id, path, settings: settings() }], report), "書き出し中");
            }}
          >
            書き出す…
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ---------- 提出前チェック ----------

export function runCheck() {
  const comp = currentComp();
  if (comp) void startTask(() => api.checkStart(comp.id), "提出前チェック");
}

export function CheckDialog() {
  const open = useOpen("check");
  const check = useUi((s) => s.check);
  const busy = useUi((s) => s.task?.[0] === "提出前チェック");
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[80vh] flex-col sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>提出前チェック</DialogTitle>
          <DialogDescription>光過敏性の検査は制作中の目安です。正式な適合検査の代わりにはなりません。</DialogDescription>
        </DialogHeader>
        <div className="-mx-6 flex min-h-0 flex-col gap-1 overflow-y-auto px-6 text-sm">
          {busy && <p className="text-muted-foreground">解析中…（全フレームを低解像度でレンダリングします）</p>}
          {!busy && !check && <p className="text-muted-foreground">「チェックを実行」を押すと、現在のコンポを調べます。</p>}
          {check?.lines.map(([ok, text], i) => (
            <p key={i} className={ok ? "text-emerald-400" : "text-amber-400"}>
              {ok ? "◆" : "⚠"} {text}
            </p>
          ))}
          {check && check.flashes.length > 0 && <p className="text-xs text-muted-foreground">明滅の区間はタイムラインに赤で表示されます</p>}
        </div>
        <DialogFooter>
          <Button disabled={busy} onClick={runCheck}>
            チェックを実行
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ---------- 取り消し履歴 ----------

export function HistoryDialog() {
  const open = useOpen("history");
  const revision = useUi((s) => s.snap?.revision);
  const [h, setH] = useState<{ undo: string[]; redo: string[] }>({ undo: [], redo: [] });
  useEffect(() => {
    if (open) void api.history().then(setH);
  }, [open, revision]);
  const jump = (index: number | null, forward: number | null) => void api.historyJump(index, forward).then(adopt).catch((e) => set({ status: String(e) }));
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[80vh] flex-col sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>履歴</DialogTitle>
          <DialogDescription>操作を押すと、その操作の直前まで戻ります。</DialogDescription>
        </DialogHeader>
        <div className="-mx-6 flex min-h-0 flex-col gap-0.5 overflow-y-auto px-4 text-sm">
          {h.undo.map((label, i) => (
            <button key={i} type="button" className="rounded-2xl px-2 py-1 text-left hover:bg-muted" onClick={() => jump(i, null)}>
              {i + 1}. {label}
            </button>
          ))}
          <div className="px-2 py-1 font-semibold">現在の状態</div>
          {h.redo.map((label, i) => (
            <button key={i} type="button" className="rounded-2xl px-2 py-1 text-left text-muted-foreground hover:bg-muted" onClick={() => jump(null, i + 1)}>
              {label}（やり直し可）
            </button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ---------- 版（スナップショット） ----------

const ago = (secs: number) => {
  const d = Math.max(0, Date.now() / 1000 - secs);
  return d < 3600 ? `${Math.floor(d / 60)} 分前` : d < 86400 ? `${Math.floor(d / 3600)} 時間前` : `${Math.floor(d / 86400)} 日前`;
};

export function VersionsDialog() {
  const open = useOpen("versions");
  const path = useUi((s) => s.snap?.path);
  const diffLabel = useUi((s) => s.diffLabel);
  const [name, setName] = useState("1次提出版");
  const [list, setList] = useState<{ name: string; saved: number; file: string }[] | null>(null);
  const reload = () => void api.versions().then(setList);
  useEffect(() => {
    if (open) reload();
  }, [open, path]);
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[80vh] flex-col sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>版（スナップショット）</DialogTitle>
          <DialogDescription>提出した時点などの状態を名前を付けて残し、あとで比べたり戻したりできます。</DialogDescription>
        </DialogHeader>
        {!path ? (
          <p className="text-muted-foreground">版を残すには、先にプロジェクトを保存してください。</p>
        ) : (
          <div className="-mx-6 flex min-h-0 flex-col gap-2 overflow-y-auto px-6">
            <div className="flex gap-2">
              <Input className="h-8" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
              <Button size="sm" disabled={!name.trim()} onClick={() => void reply(() => api.versionSave(name.trim())).then(reload)}>
                この状態を版として保存
              </Button>
            </div>
            <div className="flex gap-1">
              {["1次提出版", "2次提出版", "最終版"].map((p) => (
                <Button key={p} variant="ghost" size="sm" className="h-7 text-xs" onClick={() => setName(p)}>
                  {p}
                </Button>
              ))}
            </div>
            {list?.length === 0 && <p className="text-xs text-muted-foreground">まだ版はありません。</p>}
            {list?.map((v) => (
              <div key={v.file} className="flex items-center gap-2 rounded-3xl bg-muted/50 px-3 py-1.5 text-sm">
                <span className="flex-1 truncate">
                  {v.name}
                  <span className="pl-2 text-xs text-muted-foreground">{ago(v.saved)}</span>
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 text-xs"
                  title="いまのフレームで、この版との違いを赤く表示します"
                  onClick={async () => {
                    try {
                      const label = await api.versionCompare(v.file, v.name, get().compId, get().frame);
                      set((s) => ({ diffLabel: label, diffGen: s.diffGen + 1 }));
                    } catch (e) {
                      set({ status: String(e) });
                    }
                  }}
                >
                  現在と比較
                </Button>
                <Button variant="ghost" size="sm" className="h-7 text-xs" onClick={() => void reply(() => api.versionRestore(v.file, v.name))}>
                  この版に戻す
                </Button>
              </div>
            ))}
            {diffLabel && (
              <Button variant="secondary" size="sm" className="self-start" onClick={closeDiff}>
                比較表示を閉じる
              </Button>
            )}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

export function closeDiff() {
  void api.diffClose();
  set({ diffLabel: null });
}

// ---------- レビューコメント ----------

export function ReviewsDialog() {
  const open = useOpen("reviews");
  const comp = useUi(currentComp);
  const frame = useUi((s) => s.frame);
  const reviews = useUi((s) => (s.project?.reviews as ReviewComment[] | undefined) ?? []);
  const [text, setText] = useState("");
  if (!comp) return null;
  const fr = comp.frame_rate;
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[80vh] flex-col sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>レビューコメント</DialogTitle>
          <DialogDescription>フレームにコメントを残します。ロック中のコンポにも付けられます。</DialogDescription>
        </DialogHeader>
        <div className="flex gap-2">
          <Input className="h-8" value={text} placeholder="コメント" onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
          <Button
            size="sm"
            disabled={!text.trim()}
            onClick={() => {
              const t = text;
              setText("");
              void reply(() => api.reviewAdd(comp.id, frame, t, null));
            }}
          >
            {timecode(fr, frame)} に追加
          </Button>
        </div>
        <div className="-mx-6 flex min-h-0 flex-col gap-1 overflow-y-auto px-6 text-sm">
          {reviews.map((r, i) =>
            r.comp !== comp.id ? null : (
              <div key={i} className={cn("flex items-center gap-2", r.resolved && "text-muted-foreground")}>
                <Button variant="ghost" size="sm" className="h-7 font-mono text-xs" title="このフレームへ移動" onClick={() => setFrame(timeToFrame(fr, r.time))}>
                  {timecode(fr, timeToFrame(fr, r.time))}
                </Button>
                <span className="flex-1">
                  {r.resolved && "（解決済み）"}
                  {r.author}: {r.text}
                </span>
                {!r.resolved && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-7 text-xs"
                    onClick={() => {
                      // コメントの解決もロック中のコンポでできるよう、コンポではなくプロジェクトを編集する
                      edit("コメント解決", (p) => {
                        const list = p.reviews as ReviewComment[] | undefined;
                        if (!list?.[i]) return false;
                        list[i].resolved = true;
                      });
                    }}
                  >
                    解決
                  </Button>
                )}
              </div>
            ),
          )}
          {reviews.every((r) => r.comp !== comp.id) && <p className="text-xs text-muted-foreground">このコンポにはコメントがありません。</p>}
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ---------- ライセンス ----------

export function LicensesDialog() {
  const open = useOpen("licenses");
  const [data, setData] = useState<{ components: { name: string; license: string; usage: string }[]; notice: string } | null>(null);
  useEffect(() => {
    if (open && !data) void api.licenses().then(setData);
  }, [open]);
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="flex max-h-[80vh] flex-col sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>使用 OSS とライセンス</DialogTitle>
        </DialogHeader>
        <div className="-mx-6 flex min-h-0 flex-col gap-3 overflow-y-auto px-6 text-sm select-text">
          {data?.components.map((c) => (
            <div key={c.name}>
              <p className="font-semibold">{c.name}</p>
              <p>ライセンス: {c.license}</p>
              <p className="text-xs text-muted-foreground">{c.usage}</p>
            </div>
          ))}
          {data && <p className="text-xs whitespace-pre-wrap text-muted-foreground">{data.notice}</p>}
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ---------- 自動保存からの復旧 ----------

export function RecoveryDialog() {
  const recovery = useUi((s) => s.snap?.recovery ?? null);
  return (
    <Dialog open={recovery !== null} onOpenChange={(o) => !o && void reply(() => api.recover(false))}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>自動保存からの復旧</DialogTitle>
          <DialogDescription>保存済みのファイルより新しい自動保存が見つかりました。前回、保存せずに終了した（または異常終了した）可能性があります。</DialogDescription>
        </DialogHeader>
        <p className="font-mono text-xs break-all text-muted-foreground">{recovery}</p>
        <DialogFooter>
          <Button variant="ghost" onClick={() => void reply(() => api.recover(false))}>
            無視
          </Button>
          <Button onClick={() => void reply(() => api.recover(true))}>自動保存を復元</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** 明滅の区間（提出前チェックの結果）を、タイムラインの上に赤で出すための範囲。 */
export function useFlashes(): [number, number, string][] {
  return useUi((s) => s.check?.flashes ?? []);
}
