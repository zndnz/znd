// インスペクタの細部（znd-app のプロパティと同じ項目）: レイヤーの時間・速度・タイムリマップ・スイッチ・マット、
// 音声（ミュート・フェード・ピッチ）、テキストの書式とアニメーター、シェイプの要素、エフェクトのプリセット、
// 素材のプロキシと権利情報、プロジェクトの色の設定。

import { useEffect, useState } from "react";
import { api } from "../api";
import type { Composition, EffectInstance, FootageItem, Layer } from "../model";
import { fromSeconds, seconds, timeToFrame, frameToTime } from "../model";
import { adopt, computeLayer, edit, editLayer, propOp, reply, set, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Choice, IconButton, NumberField, Row, Section, TextField } from "./controls";

const toHex = (c: number[]) =>
  "#" + c.slice(0, 3).map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, "0")).join("");
const fromHex = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);

let catalog: Promise<{ shape_items: [string, string][]; animator_props: [string, string][] }> | null = null;
const useCatalog = () => {
  const [c, setC] = useState<{ shape_items: [string, string][]; animator_props: [string, string][] } | null>(null);
  useEffect(() => void (catalog ??= api.inspectorCatalog()).then(setC), []);
  return c;
};

const flag = (layer: Layer, key: string) => !!(layer as Record<string, unknown>)[key];

// ---------- レイヤー ----------

/** スイッチ（ガイド・コラップス・フレームブレンド・ライト）と、マットの種類。 */
export function LayerFlags({ layer }: { layer: Layer }) {
  const toggle = (key: string, label: string) => editLayer(label, layer.id, (l) => void ((l as Record<string, unknown>)[key] = !flag(l, key)));
  const items: [string, string, boolean][] = [
    ["guide", "ガイド（書き出さない）", true],
    ["collapse", "コラップス", layer.kind.type === "precomp"],
    ["frame_blend", "フレームブレンド", layer.kind.type === "footage"],
    ["accepts_lights", "ライトを受ける", !!layer.three_d],
    ["casts_shadows", "影を落とす", !!layer.three_d],
  ];
  const matte = layer.track_matte as { layer: string; mode: string } | null | undefined;
  return (
    <>
      {matte && (
        <Row label="マットの種類">
          <Choice
            value={matte.mode}
            options={[
              ["alpha", "アルファ"],
              ["alpha_inverted", "アルファ反転"],
              ["luma", "ルミナンス"],
              ["luma_inverted", "ルミナンス反転"],
            ]}
            onChange={(v) => editLayer("マットモード", layer.id, (l) => void ((l.track_matte as { mode: string }).mode = v))}
          />
        </Row>
      )}
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        {items
          .filter(([, , show]) => show)
          .map(([key, label]) => (
            <label key={key} className="flex min-h-7 items-center gap-2 text-sm">
              <Checkbox checked={flag(layer, key)} onCheckedChange={() => toggle(key, label)} />
              {label}
            </label>
          ))}
      </div>
    </>
  );
}

/** イン点・アウト点・ソースの開始（フレーム）、速度、タイムリマップ、音声の設定。 */
export function LayerTiming({ comp, layer }: { comp: Composition; layer: Layer }) {
  const fr = comp.frame_rate;
  const inF = timeToFrame(fr, layer.in_point);
  const outF = timeToFrame(fr, layer.out_point);
  const timed = ["footage", "precomp", "audio"].includes(layer.kind.type);
  const speed = ((layer.speed as number | undefined) ?? 1) * 100;
  const audio = layer.audio as { mute?: boolean; fade_in?: number; fade_out?: number; pitch?: number } | undefined;
  const hasAudio = ["footage", "audio", "precomp"].includes(layer.kind.type);
  const setAudio = (p: Record<string, unknown>, d: boolean) => editLayer("オーディオ設定", layer.id, (l) => void (l.audio = { ...(l.audio as object), ...p }), d);
  return (
    <>
      <Row label="イン／アウト" title="フレーム">
        <NumberField value={inF} digits={0} onChange={(v, d) => editLayer("イン点", layer.id, (l) => void (l.in_point = frameToTime(fr, Math.min(Math.round(v), outF - 1))), d)} />
        <NumberField value={outF} digits={0} onChange={(v, d) => editLayer("アウト点", layer.id, (l) => void (l.out_point = frameToTime(fr, Math.max(Math.round(v), inF + 1))), d)} />
      </Row>
      <Row label="ソースの開始" title="ソースの先頭が来るフレーム">
        <NumberField value={timeToFrame(fr, layer.start_time)} digits={0} onChange={(v, d) => editLayer("開始位置", layer.id, (l) => void (l.start_time = frameToTime(fr, Math.round(v))), d)} />
      </Row>
      {timed && (
        <>
          <Row label="速度" title="映像と音声の両方に掛かります（音程は変わりません）">
            <NumberField value={speed} digits={1} min={1} max={1000} unit="%" onChange={(v, d) => editLayer("再生速度", layer.id, (l) => void (l.speed = v / 100), d)} />
          </Row>
          {layer.time_remap == null && (
            <Button variant="secondary" size="sm" className="self-start" onClick={() => void propOp("タイムリマップ", layer.id, "time_remap", { op: "add_key" })}>
              タイムリマップを使う
            </Button>
          )}
        </>
      )}
      {hasAudio && audio && (
        <>
          <label className="flex min-h-8 items-center gap-2 text-sm">
            <Checkbox checked={!!audio.mute} onCheckedChange={(on) => setAudio({ mute: on }, false)} />
            ミュート
          </label>
          <Row label="フェード">
            <NumberField value={seconds(audio.fade_in ?? 0)} digits={2} step={0.01} min={0} max={60} unit="秒" onChange={(v, d) => setAudio({ fade_in: fromSeconds(v) }, d)} />
            <NumberField value={seconds(audio.fade_out ?? 0)} digits={2} step={0.01} min={0} max={60} unit="秒" onChange={(v, d) => setAudio({ fade_out: fromSeconds(v) }, d)} />
          </Row>
          <Row label="ピッチ">
            <NumberField value={audio.pitch ?? 0} digits={1} step={0.1} min={-24} max={24} unit="半音" onChange={(v, d) => setAudio({ pitch: v }, d)} />
          </Row>
        </>
      )}
    </>
  );
}

// ---------- テキスト ----------

interface Doc {
  text: string;
  font_family: string;
  weight?: number;
  size: number;
  fill: number[] | null;
  stroke?: { color: number[]; width: number } | null;
  tracking?: number;
  justify?: string;
  vertical?: boolean;
  box_size?: [number, number] | null;
}

/** テキストの書式（太さ・線・揃え・縦書き・字間・段落）と、見つからないフォントの注意。 */
export function TextFormat({ layer }: { layer: Layer }) {
  const doc = (layer.kind as { document: Doc }).document;
  const [missing, setMissing] = useState(false);
  useEffect(() => void (doc.font_family ? api.fontAvailable(doc.font_family).then((ok) => setMissing(!ok)) : setMissing(false)), [doc.font_family]);
  const change = (label: string, f: (d: Doc) => void, d = false) => editLayer(label, layer.id, (l) => void f((l.kind as { document: Doc }).document), d);
  return (
    <>
      {missing && <p className="text-xs text-destructive">フォント「{doc.font_family}」が見つかりません（代替フォントで表示中）</p>}
      <Row label="太さ">
        <NumberField value={doc.weight ?? 400} digits={0} step={10} min={100} max={900} onChange={(v, d) => change("文字の太さ", (x) => void (x.weight = Math.round(v / 100) * 100 || 100), d)} />
      </Row>
      <Row label="線">
        <Checkbox checked={!!doc.stroke} onCheckedChange={(on) => change("文字の線", (x) => void (x.stroke = on ? { color: [0, 0, 0, 1], width: 4 } : null))} />
        {doc.stroke && (
          <>
            <input
              type="color"
              className="h-8 w-12 rounded-3xl"
              value={toHex(doc.stroke.color)}
              onChange={(e) => change("線の色", (x) => void (x.stroke = { ...x.stroke!, color: [...fromHex(e.target.value), x.stroke!.color[3] ?? 1] }), true)}
            />
            <NumberField value={doc.stroke.width} min={0} max={200} unit="px" onChange={(v, d) => change("線の幅", (x) => void (x.stroke = { ...x.stroke!, width: v }), d)} />
          </>
        )}
      </Row>
      <Row label="揃え">
        <Choice
          value={doc.justify ?? "left"}
          options={[
            ["left", "左"],
            ["center", "中央"],
            ["right", "右"],
          ]}
          onChange={(v) => change("揃え", (x) => void (x.justify = v))}
        />
        <label className="flex shrink-0 items-center gap-1.5 text-sm">
          <Checkbox checked={!!doc.vertical} onCheckedChange={(on) => change("縦書き", (x) => void (x.vertical = on))} />
          縦書き
        </label>
      </Row>
      <Row label="字間">
        <NumberField value={doc.tracking ?? 0} onChange={(v, d) => change("字間", (x) => void (x.tracking = v), d)} />
      </Row>
      <Row label="段落" title="幅を決めて自動で折り返します">
        <Checkbox checked={!!doc.box_size} onCheckedChange={(on) => change("段落", (x) => void (x.box_size = on ? [800, 400] : null))} />
        {doc.box_size && <NumberField value={doc.box_size[0]} min={10} max={8000} unit="px" onChange={(v, d) => change("段落の幅", (x) => void (x.box_size = [v, x.box_size![1]]), d)} />}
      </Row>
    </>
  );
}

const UNITS: [string, string][] = [
  ["characters", "文字ごと"],
  ["characters_excluding_spaces", "文字ごと（空白を除く）"],
  ["words", "単語ごと"],
  ["lines", "行ごと"],
];

/** テキストアニメーター: 空のアニメーターを足し、属性を選んで、下の値にキーフレームを付ける。 */
export function AnimatorList({ layer }: { layer: Layer }) {
  const cat = useCatalog();
  const [list, setList] = useState<{ name: string; props: string[]; unit: string | null }[]>([]);
  useEffect(() => void api.animators(layer).then(setList), [layer]);
  const run = (label: string, op: Record<string, unknown> & { op: string }) => void computeLayer(label, layer.id, (l) => api.contentOp(l, op));
  return (
    <div className="flex flex-col gap-1.5">
      <Button
        variant="secondary"
        size="sm"
        className="self-start"
        title="文字ごとに動きを付けるアニメーターを足します。属性を選んでから、値やキーフレームを付けてください"
        onClick={() => run("アニメーター追加", { op: "add_animator" })}
      >
        ＋ アニメーター
      </Button>
      {list.map((a, i) => (
        <div key={i} className="flex items-center gap-1.5">
          <span className="flex-1 truncate text-sm">{a.name}</span>
          <DropdownMenu>
            <DropdownMenuTrigger render={<Button variant="ghost" size="sm" className="h-7 px-2 text-xs" />}>属性（{a.props.length}）</DropdownMenuTrigger>
            <DropdownMenuContent className="w-52">
              {cat?.animator_props.map(([key, label]) => (
                <DropdownMenuCheckboxItem key={key} checked={a.props.includes(key)} onCheckedChange={(on) => run("アニメーターの属性", { op: "animator_prop", index: i, key, on })}>
                  {label}
                </DropdownMenuCheckboxItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
          {a.unit && <Choice className="w-36" value={a.unit} options={UNITS} onChange={(v) => run("アニメーターの単位", { op: "animator_unit", index: i, unit: v })} />}
          <IconButton icon="close" title="このアニメーターを削除" size="icon-xs" onClick={() => run("アニメーター削除", { op: "remove_animator", index: i })} />
        </div>
      ))}
    </div>
  );
}

// ---------- シェイプ ----------

export function ShapeItems({ layer }: { layer: Layer }) {
  const cat = useCatalog();
  const n = ((layer.kind as { items?: unknown[] }).items ?? []).length;
  const run = (label: string, op: Record<string, unknown> & { op: string }) => void computeLayer(label, layer.id, (l) => api.contentOp(l, op));
  return (
    <div className="flex items-center gap-2">
      <span className="flex-1 text-xs text-muted-foreground">要素 {n} 個</span>
      <DropdownMenu>
        <DropdownMenuTrigger render={<Button variant="secondary" size="sm" />}>要素を追加</DropdownMenuTrigger>
        <DropdownMenuContent className="w-52">
          {cat?.shape_items.map(([key, label]) => (
            <DropdownMenuItem key={key} onClick={() => run("シェイプ要素追加", { op: "add_shape_item", key })}>
              {label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      {n > 0 && (
        <Button variant="ghost" size="sm" onClick={() => run("シェイプ要素削除", { op: "remove_last_shape_item" })}>
          末尾を削除
        </Button>
      )}
    </div>
  );
}

// ---------- エフェクトのプリセット ----------

export function EffectPresets({ layer }: { layer: Layer }) {
  const effects = (layer.effects ?? []) as EffectInstance[];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button variant="ghost" size="sm" className="h-7 px-2 text-xs text-muted-foreground" />}>プリセット</DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuItem disabled={effects.length === 0} onClick={() => void reply(() => api.effectPresetSave(layer.name, effects))}>
          保存…
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={async () => {
            try {
              const fx = await api.effectPresetLoad();
              if (fx) editLayer("プリセット適用", layer.id, (l) => void (l.effects = [...(l.effects ?? []), ...fx]));
            } catch (e) {
              set({ status: String(e) });
            }
          }}
        >
          読み込んで追加…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ---------- 素材 ----------

const CLEARANCES: [string, string][] = [
  ["unconfirmed", "未確認"],
  ["cleared", "許諾済み"],
  ["not_applicable", "該当なし"],
  ["denied", "不許可"],
];

interface Rights {
  author?: string;
  source_url?: string;
  license?: string;
  cleared_on?: string;
  copyright?: string;
  neighboring?: string;
  portrait?: string;
  trademark?: string;
  notes?: string;
}

/** 素材のプロキシ・可変フレームレートの注意・権利情報。 */
export function FootageDetails({ item }: { item: FootageItem }) {
  const r = (item.rights as Rights | undefined) ?? {};
  const setR = (p: Partial<Rights>) =>
    edit("権利情報", (proj) => {
      const f = proj.footage.find((x) => x.id === item.id);
      if (!f) return false;
      f.rights = { ...(f.rights as Rights), ...p };
    });
  return (
    <>
      {!!item.vfr && <p className="text-xs text-amber-400">可変フレームレート素材（固定レートに適合して読み込み）</p>}
      {item.frame_rate && (
        <Button
          variant="secondary"
          size="sm"
          className="self-start"
          title="1/2 解像度の代替ファイルを作り、プレビューを軽くします"
          onClick={async () => {
            try {
              const [msg, snap] = await api.makeProxy(item.id);
              set({ status: msg });
              adopt(snap);
            } catch (e) {
              set({ status: String(e) });
            }
          }}
        >
          プロキシを生成
        </Button>
      )}
      <p className="pt-2 text-xs text-muted-foreground">権利情報</p>
      {(
        [
          ["author", "作者"],
          ["source_url", "入手元 URL"],
          ["license", "ライセンス"],
          ["cleared_on", "許諾日"],
        ] as [keyof Rights, string][]
      ).map(([k, label]) => (
        <Row key={k} label={label}>
          <TextField value={r[k] ?? ""} onChange={(v) => setR({ [k]: v })} />
        </Row>
      ))}
      {(
        [
          ["copyright", "著作権"],
          ["neighboring", "著作隣接権・原盤権"],
          ["portrait", "肖像権"],
          ["trademark", "商標"],
        ] as [keyof Rights, string][]
      ).map(([k, label]) => (
        <Row key={k} label={label}>
          <Choice value={r[k] ?? "unconfirmed"} options={CLEARANCES} onChange={(v) => setR({ [k]: v })} />
        </Row>
      ))}
      <Row label="利用条件メモ">
        <TextField multiline value={r.notes ?? ""} onChange={(v) => setR({ notes: v })} />
      </Row>
      <Button
        variant="ghost"
        size="sm"
        className="self-start"
        onClick={() => setR({ copyright: "not_applicable", neighboring: "not_applicable", portrait: "not_applicable", trademark: "not_applicable" })}
      >
        自作素材（すべて該当なし）にする
      </Button>
    </>
  );
}

// ---------- プロジェクト ----------

const SPACES: [string, string][] = [
  ["srgb", "sRGB"],
  ["rec709", "Rec.709"],
  ["display_p3", "Display P3"],
  ["aces_cg", "ACEScg"],
];

/** プロジェクトの色の設定（リニアライト合成・色深度・作業色空間・出力色空間）。 */
export function ProjectSettings() {
  const st = useUi((s) => s.project?.settings as { linear_blending?: boolean; bit_depth?: string; working_space?: string; output_space?: string } | undefined);
  if (!st) return null;
  const change = (label: string, p: Record<string, unknown>) => edit(label, (proj) => void (proj.settings = { ...(proj.settings as object), ...p }));
  return (
    <Section title="プロジェクトの色" defaultOpen={false}>
      <label className="flex min-h-8 items-center gap-2 text-sm">
        <Checkbox checked={!!st.linear_blending} onCheckedChange={(on) => change("合成設定", { linear_blending: on })} />
        リニアライト合成
      </label>
      <Row label="色深度">
        <Choice
          value={st.bit_depth ?? "32f"}
          options={[
            ["8", "8bpc"],
            ["16", "16bpc"],
            ["32f", "32bpc（浮動小数）"],
          ]}
          onChange={(v) => change("色深度", { bit_depth: v })}
        />
      </Row>
      <Row label="作業色空間">
        <Choice value={st.working_space ?? "srgb"} options={SPACES} onChange={(v) => change("作業色空間", { working_space: v })} />
      </Row>
      <Row label="出力">
        <Choice value={st.output_space ?? "srgb"} options={SPACES} onChange={(v) => change("出力色空間", { output_space: v })} />
      </Row>
    </Section>
  );
}
