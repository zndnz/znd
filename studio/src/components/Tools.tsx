// 「ビートに合わせる」とキーボードショートカットのウィンドウ、インスペクタに足す区画
// （コンポのビートグリッドとロック、映像と音のずれの補正、レイヤーの整列）。

import { useEffect, useState, useSyncExternalStore } from "react";
import { api } from "../api";
import { keyTarget, runOp } from "../actions";
import { ACTIONS, chordOfEvent, chordsOf, conflicts, display, keymap, normalize, onKeymapChange, PRESETS, type Preset, saveKeymap } from "../keymap";
import type { Composition, Layer } from "../model";
import { fromSeconds, seconds } from "../model";
import { currentComp, editComp, set, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Choice, IconButton, NumberField, Row, Section } from "./controls";

const close = () => set({ dialog: null });

// ---------- ビートに合わせる ----------

let beatUi = { source: 0, workOnly: true, delta: 15, frames: 6 };

export function BeatfitDialog() {
  const open = useUi((s) => s.dialog === "beatfit");
  const comp = useUi(currentComp);
  const layer = useUi((s) => currentComp(s)?.layers.find((l) => l.id === s.selected));
  const work = useUi((s) => s.workArea);
  const revision = useUi((s) => s.snap?.revision);
  const [sources, setSources] = useState<string[]>([]);
  const [ui, setUi] = useState(beatUi);
  const [count, setCount] = useState(0);
  const patch = (p: Partial<typeof ui>) => setUi((u) => (beatUi = { ...u, ...p }));
  const range = ui.workOnly && work ? work : null;
  useEffect(() => void api.beatSources().then(setSources), []);
  useEffect(() => {
    if (open && comp) void api.beatCount(comp.id, layer?.id ?? null, ui.source, range).then(setCount);
  }, [open, comp?.id, layer?.id, ui.source, range?.[0], range?.[1], revision]);
  if (!comp) return null;
  const path = keyTarget?.layer === layer?.id && keyTarget ? keyTarget.path : "transform.scale";
  const ready = count > 0 && !!layer && !layer.locked;
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>ビートに合わせる</DialogTitle>
          <DialogDescription>ビートやマーカーの位置にレイヤーを並べたり、パルスのキーフレームを打ったりします。</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2">
          <Row label="ビート">
            <Choice value={String(ui.source)} options={sources.map((s, i): [string, string] => [String(i), s])} onChange={(v) => patch({ source: Number(v) })} />
          </Row>
          <label className="flex min-h-8 items-center gap-2">
            <Checkbox checked={ui.workOnly} onCheckedChange={(on) => patch({ workOnly: on })} />
            ワークエリアの中だけ（B / N で設定）
          </label>
          {!comp.beat_grid && ui.source <= 3 ? (
            <p className="text-xs text-amber-400">このコンポにはビートグリッドがありません（コンポのインスペクタで BPM を指定してください）</p>
          ) : (
            <p className="text-xs text-muted-foreground">対象のビート: {count} 個</p>
          )}
          <p className="pt-2 font-semibold">選択レイヤーを並べる</p>
          <p className="text-xs text-muted-foreground">{layer ? `対象: ${layer.name}` : "レイヤーを選択してください"}</p>
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="secondary"
              disabled={!ready}
              title="ビートごとに複製し、毎回素材の同じ位置から再生します。音MAD の連打・頭出し"
              onClick={() => void runOp({ op: "stamp", layer: layer!.id, source: ui.source, range, restart: true })}
            >
              頭出しで並べる（連打）
            </Button>
            <Button
              size="sm"
              variant="secondary"
              disabled={!ready}
              title="素材は流れたまま、ビートの位置で切れ目を入れます"
              onClick={() => void runOp({ op: "stamp", layer: layer!.id, source: ui.source, range, restart: false })}
            >
              ビートで切る（続けて再生）
            </Button>
          </div>
          <p className="pt-2 font-semibold">パルス（ビートで跳ねて戻る）</p>
          <p className="text-xs text-muted-foreground">対象のプロパティ: {path}（タイムラインでキーをクリックすると変えられます）</p>
          <Row label="変化量">
            <NumberField value={ui.delta} step={0.5} onChange={(v) => patch({ delta: v })} />
          </Row>
          <Row label="戻るまで">
            <NumberField value={ui.frames} digits={0} min={1} max={120} unit="フレーム" onChange={(v) => patch({ frames: Math.round(v) })} />
          </Row>
          <Button
            size="sm"
            className="self-start"
            disabled={!ready}
            onClick={() => void runOp({ op: "pulse", layer: layer!.id, source: ui.source, range, path, delta: ui.delta, frames: ui.frames })}
          >
            キーフレームを打つ
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ---------- キーボードショートカット ----------

const useKeymap = () => useSyncExternalStore(onKeymapChange, () => keymap);

export function ShortcutsDialog() {
  const open = useUi((s) => s.dialog === "shortcuts");
  const km = useKeymap();
  const [capture, setCapture] = useState<string | null>(null);
  const save = (next: typeof km) => void saveKeymap(next).then((m) => set({ status: m }), (e) => set({ status: `ショートカットを保存できません: ${e}` }));
  useEffect(() => {
    if (!capture) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const chord = chordOfEvent(e);
      if (!chord) return;
      setCapture(null);
      if (chord === "Escape") return;
      const cur = chordsOf(capture);
      if (!cur.some((c) => normalize(c) === chord)) save({ ...km, bindings: { ...km.bindings, [capture]: [...cur, chord] } });
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [capture, km]);
  const clash = conflicts();
  const clashKeys = new Set(clash.map(([c]) => c));
  let group = "";
  return (
    <Dialog open={open} onOpenChange={(o) => !o && (setCapture(null), close())}>
      <DialogContent className="flex max-h-[86vh] flex-col sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>キーボードショートカット</DialogTitle>
          <DialogDescription>znd-app と同じ設定ファイル（keymap.json）を使います。キーを押すと外し、＋で追加します。</DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-2">
          <span className="text-sm text-muted-foreground">プリセット</span>
          <Choice className="w-60" value={km.preset} options={PRESETS} onChange={(v) => save({ preset: v as Preset, bindings: {} })} />
          <Button variant="ghost" size="sm" disabled={Object.keys(km.bindings).length === 0} onClick={() => save({ ...km, bindings: {} })}>
            変更をすべて戻す
          </Button>
        </div>
        {clash.map(([c, ids]) => (
          <p key={c} className="text-xs text-amber-400">
            {display(c)} が重複: {ids.map((id) => ACTIONS.find((a) => a.id === id)?.label).join("、")}（一覧の上にあるものが優先）
          </p>
        ))}
        {capture && <p className="text-xs text-sky-400">「{ACTIONS.find((a) => a.id === capture)?.label}」に割り当てるキーを押してください（Esc で取り消し）</p>}
        <div className="-mx-6 flex min-h-0 flex-col overflow-y-auto px-6 text-sm">
          {ACTIONS.map((a) => {
            const head = a.group !== group;
            group = a.group;
            const chords = chordsOf(a.id);
            return (
              <div key={a.id}>
                {head && <p className="pt-3 pb-1 font-semibold">{a.group}</p>}
                <div className="flex min-h-8 items-center gap-2">
                  <span className="w-72 shrink-0">{a.label}</span>
                  <span className="flex flex-1 flex-wrap gap-1">
                    {chords.map((c, i) => (
                      <Button
                        key={c}
                        variant="secondary"
                        size="sm"
                        className={`h-6 px-2 font-mono text-xs ${clashKeys.has(normalize(c) ?? "") ? "text-amber-400" : ""}`}
                        title="クリックで外す"
                        onClick={() => save({ ...km, bindings: { ...km.bindings, [a.id]: chords.filter((_, k) => k !== i) } })}
                      >
                        {display(c)}
                      </Button>
                    ))}
                    {chords.length === 0 && <span className="text-xs text-muted-foreground">なし</span>}
                  </span>
                  <IconButton icon="plus" title="キーを追加" size="icon-xs" onClick={() => setCapture(a.id)} />
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-6 px-2 text-xs"
                    disabled={!(a.id in km.bindings)}
                    title="プリセットの割り当てに戻す"
                    onClick={() => {
                      const b = { ...km.bindings };
                      delete b[a.id];
                      save({ ...km, bindings: b });
                    }}
                  >
                    既定
                  </Button>
                </div>
              </div>
            );
          })}
          <p className="pt-4 pb-1 font-semibold">マウス操作</p>
          {[
            ["Shift + ドラッグ", "スナップせずに移動"],
            ["キーをクリック / Shift + クリック", "キーフレームを選択 / 選択に足す・外す"],
            ["選択キーをドラッグ", "まとめて移動"],
            ["⌘C / ⌘V", "キーフレームをコピー／再生ヘッドの位置に貼り付け"],
            ["マーカーをダブルクリック", "コメントを編集（右クリックで削除）"],
            ["⌘ + ホイール", "タイムラインを拡大・縮小（ホイールだけで横スクロール）"],
          ].map(([k, w]) => (
            <div key={k} className="flex min-h-7 items-center gap-2">
              <span className="w-72 shrink-0 font-mono text-xs">{k}</span>
              <span>{w}</span>
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ---------- インスペクタの区画 ----------

interface BeatGrid {
  bpm: number;
  offset: number;
  beats_per_bar: number;
  subdivision: number;
}

/** コンポのビートグリッドとロック。 */
export function CompExtras({ comp }: { comp: Composition }) {
  const g = comp.beat_grid as BeatGrid | null | undefined;
  const grid: BeatGrid = g ?? { bpm: 120, offset: 0, beats_per_bar: 4, subdivision: 4 };
  const setGrid = (p: Partial<BeatGrid> | null, d = false) => editComp("ビートグリッド", (c) => void (c.beat_grid = p === null ? null : { ...grid, ...p }), d);
  const locked = comp.locked_by as string | null | undefined;
  const markers = (comp.markers as unknown[] | undefined)?.length ?? 0;
  return (
    <>
      <Section title="ビートグリッド" defaultOpen={!!g}>
        <Row label="使う">
          <Checkbox checked={!!g} onCheckedChange={(on) => setGrid(on ? {} : null)} />
        </Row>
        {g && (
          <>
            <Row label="BPM">
              <NumberField value={grid.bpm} digits={2} step={0.1} min={20} max={400} onChange={(v, d) => setGrid({ bpm: v }, d)} />
            </Row>
            <Row label="拍子">
              <NumberField value={grid.beats_per_bar} digits={0} min={1} max={16} unit="拍" onChange={(v, d) => setGrid({ beats_per_bar: Math.round(v) }, d)} />
            </Row>
            <Row label="分割">
              <NumberField value={grid.subdivision} digits={0} min={1} max={16} onChange={(v, d) => setGrid({ subdivision: Math.round(v) }, d)} />
            </Row>
            <Row label="開始">
              <NumberField value={seconds(grid.offset)} digits={3} step={0.001} unit="秒" onChange={(v, d) => setGrid({ offset: fromSeconds(v) }, d)} />
            </Row>
          </>
        )}
        <p className="text-xs text-muted-foreground/70">
          マーカー {markers} 個。再生しながら M で拍を叩き、アニメーション → オーディオから生成 → BPM を推定、でも設定できます。
        </p>
      </Section>
      <Section title="共同作業" defaultOpen={!!locked}>
        {locked ? (
          <Row label="ロック">
            <span className="flex-1 text-sm">{locked} がロック中</span>
            <Button variant="secondary" size="sm" onClick={() => void runOp({ op: "lock_comp", on: false })}>
              解除
            </Button>
          </Row>
        ) : (
          <Button variant="secondary" size="sm" className="self-start" title="ほかの人が編集できないようにします（プロジェクトファイルに記録）" onClick={() => void runOp({ op: "lock_comp", on: true })}>
            このコンポをロック
          </Button>
        )}
      </Section>
      <AudioSync />
    </>
  );
}

/** 映像と音のずれの補正と、同期テスト用のコンポ。 */
function AudioSync() {
  const [st, setSt] = useState<{ offset_ms: number; latency_ms: number | null } | null>(null);
  useEffect(() => void api.audioSettings().then(setSt), []);
  if (!st) return null;
  return (
    <Section title="映像と音のずれ" defaultOpen={false}>
      {st.latency_ms !== null && <p className="text-xs text-muted-foreground">出力の遅延（自動で補正済み）: {st.latency_ms.toFixed(0)} ms</p>}
      <Row label="補正" title="正の値で映像を遅らせます。Bluetooth のイヤホンなど、音が遅れて聞こえるときに使います">
        <NumberField
          value={st.offset_ms}
          digits={0}
          min={-500}
          max={500}
          unit="ms"
          onChange={(v, d) => {
            setSt({ ...st, offset_ms: v });
            void api.setAvOffset(v, !d).catch((e) => set({ status: String(e) }));
          }}
        />
      </Row>
      <Button
        variant="secondary"
        size="sm"
        className="self-start"
        title="120 BPM のクリック音と、拍ごとに光る画面のコンポを作ります"
        onClick={() => void runOp({ op: "sync_test" })}
      >
        同期テスト用のコンポを作る
      </Button>
    </Section>
  );
}

/** レイヤーをコンポの端・中央に揃える。 */
export function AlignRow({ layer }: { layer: Layer }) {
  const go = (horizontal: boolean, mode: number) => void runOp({ op: "align", layer: layer.id, horizontal, mode });
  return (
    <Row label="整列">
      {[
        [true, -1, "左"],
        [true, 0, "左右中央"],
        [true, 1, "右"],
        [false, -1, "上"],
        [false, 0, "上下中央"],
        [false, 1, "下"],
      ].map(([h, m, label]) => (
        <Button key={String(label)} variant="ghost" size="sm" className="h-7 px-1.5 text-xs" onClick={() => go(h as boolean, m as number)}>
          {label as string}
        </Button>
      ))}
    </Row>
  );
}
