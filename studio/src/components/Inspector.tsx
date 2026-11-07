// インスペクタ（右）。選んだものの設定をすべてここに出す。
// レイヤー → その全設定、素材 → その情報、どちらも選んでいなければ現在のコンポの設定。

import { useEffect, useState } from "react";
import { api } from "../api";
import {
  BLEND_MODES,
  type Composition,
  type EffectInfo,
  type EffectInstance,
  FRAME_RATES,
  fromSeconds,
  type Layer,
  type ParamValue,
  type PropInfo,
  seconds,
  timecode,
  timeToFrame,
} from "../model";
import { currentComp, editComp, editLayer, get, run, selectedLayer, set, useUi } from "../store";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Command, CommandEmpty, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Choice, IconButton, NumberField, Row, Section, TextField } from "./controls";
import { EasingPanel } from "./Easing";
import { FontPicker } from "./FontPicker";
import { Icon } from "./icons";
import { layerActions } from "./layerActions";
import { PropRow, useLayerProps } from "./PropRow";
import { AlignRow, CompExtras } from "./Tools";

/** 選択欄の「なし」。 */
const NONE = "__none";

const toHex = (c: number[]) =>
  "#" + c.slice(0, 3).map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, "0")).join("");
const fromHex = (h: string): [number, number, number] => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255) as [number, number, number];

export function Inspector() {
  const layer = useUi(selectedLayer);
  const footage = useUi((s) => s.project?.footage.find((f) => f.id === s.selectedFootage));
  const comp = useUi(currentComp);
  if (!comp) return <aside className="inspector" />;
  return (
    <aside className="inspector">
      {layer ? <LayerInspector comp={comp} layer={layer} /> : footage ? <FootageInspector id={footage.id} /> : <CompInspector comp={comp} />}
    </aside>
  );
}

// ---------- レイヤー ----------

/** 3D レイヤーでないときは出さないプロパティ。 */
const ONLY_3D = ["transform.position_z", "transform.anchor_z", "transform.rotation_x", "transform.rotation_y"];

function LayerInspector({ comp, layer }: { comp: Composition; layer: Layer }) {
  const props = useLayerProps(layer);
  const hasKeys = useUi((s) => s.selectedKeys?.layer === layer.id);
  // キーフレームを選んだら、イージングを開く
  const [easeOpen, setEaseOpen] = useState(hasKeys);
  useEffect(() => {
    if (hasKeys) setEaseOpen(true);
  }, [hasKeys]);
  const a = layerActions(layer.id);
  const fr = comp.frame_rate;
  const inF = timeToFrame(fr, layer.in_point);
  const outF = timeToFrame(fr, layer.out_point);
  const others = comp.layers.filter((l) => l.id !== layer.id);
  const three = !!layer.three_d || ["camera", "light", "model"].includes(layer.kind.type);
  const group = (f: (p: PropInfo) => boolean) => props.filter(f);
  const transform = group((p) => (p.path.startsWith("transform.") && (three || !ONLY_3D.includes(p.path))) || /^(camera|light)\./.test(p.path));
  const hasAudio = ["footage", "audio"].includes(layer.kind.type);
  const timing = group((p) => p.path === "time_remap" || (p.path === "audio.level" && hasAudio));
  const content = group((p) => p.path.startsWith("shape."));
  const animators = group((p) => p.path.startsWith("text."));
  const masks = group((p) => p.path.startsWith("mask."));
  const rows = (list: PropInfo[]) => list.map((p) => <PropRow key={p.path} layer={layer} prop={p} />);

  return (
    <>
      <div className="flex flex-col gap-2 px-4 pt-3.5 pb-3">
        <TextField className="h-9 text-[15px] font-semibold" value={layer.name} onChange={(v) => editLayer("名前", layer.id, (l) => void (l.name = v))} />
        <div className="flex items-center gap-0.5">
          <IconButton icon={layer.visible === false ? "eyeOff" : "eye"} title="表示" on={layer.visible !== false} onClick={a.toggleVisible} />
          <IconButton icon="solo" title="ソロ" on={!!layer.solo} onClick={a.toggleSolo} />
          <IconButton icon={layer.locked ? "lock" : "unlock"} title="ロック" on={!!layer.locked} onClick={a.toggleLock} />
          <IconButton icon="threeD" title="3D レイヤー" on={!!layer.three_d} onClick={a.toggle3d} />
          <IconButton icon="blur" title="モーションブラー" on={!!layer.motion_blur} onClick={a.toggleMotionBlur} />
          <IconButton icon="shy" title="シャイ（タイムラインで隠せる）" on={!!layer.shy} onClick={a.toggleShy} />
          <span className="flex-1" />
          <IconButton icon="trash" title="削除" onClick={a.remove} />
        </div>
        <div className="text-xs tabular-nums text-muted-foreground" title="イン点 – アウト点（長さ）">
          {timecode(fr, inF)} – {timecode(fr, outF)}（{timecode(fr, outF - inF)}）
        </div>
      </div>

      <fieldset disabled={!!layer.locked} className="plain">
        <Section title="イージング" open={easeOpen} onOpenChange={setEaseOpen}>
          <EasingPanel />
        </Section>

        <KindSection layer={layer} />

        <Section title="トランスフォーム">
          {rows(transform)}
          <AlignRow layer={layer} />
          <p className="text-xs text-muted-foreground/70">◆ でキーフレームを打ちます（右クリックでアニメーションを止める）。= でエクスプレッション。</p>
        </Section>
        {timing.length > 0 && <Section title="時間・音声">{rows(timing)}</Section>}
        {content.length > 0 && <Section title="コンテンツ">{rows(content)}</Section>}
        {animators.length > 0 && <Section title="テキストアニメーター">{rows(animators)}</Section>}
        {masks.length > 0 && <Section title="マスク">{rows(masks)}</Section>}

        <Section title="合成">
          <Row label="描画モード">
            <Choice value={layer.blend_mode ?? "normal"} options={BLEND_MODES} onChange={(v) => editLayer("描画モード", layer.id, (l) => void (l.blend_mode = v))} />
          </Row>
          <Row label="親">
            <Choice
              value={layer.parent ?? NONE}
              options={[[NONE, "なし"], ...others.map((o): [string, string] => [o.id, o.name])]}
              onChange={(v) => a.parent(v === NONE ? null : v)}
            />
          </Row>
          <Row label="コマ打ち">
            <Choice
              value={String((layer.hold_frames as number | undefined) ?? 1)}
              options={[["1", "なし"], ["2", "2 コマ"], ["3", "3 コマ"], ["4", "4 コマ"]]}
              onChange={(v) => a.hold(Number(v))}
            />
          </Row>
          <Row label="トラックマット">
            <Choice
              value={(layer.track_matte as { layer: string } | null | undefined)?.layer ?? NONE}
              options={[[NONE, "なし"], ...others.map((o): [string, string] => [o.id, o.name])]}
              onChange={(v) =>
                editLayer("トラックマット", layer.id, (l) => {
                  const cur = l.track_matte as { layer: string; mode: string } | null | undefined;
                  l.track_matte = v !== NONE ? { layer: v, mode: cur?.mode ?? "alpha" } : null;
                })
              }
            />
          </Row>
        </Section>

        <EffectsSection comp={comp} layer={layer} props={props} />
      </fieldset>
    </>
  );
}

/** 種類ごとの設定（テキストの本文・平面の色・ライトの色など）。 */
function KindSection({ layer }: { layer: Layer }) {
  const k = layer.kind;
  if (k.type === "text") {
    const doc = k.document;
    return (
      <Section title="テキスト">
        <TextField
          multiline
          value={doc.text}
          onChange={(v) => editLayer("テキスト", layer.id, (l) => void (l.kind.type === "text" && (l.kind.document.text = v)))}
        />
        <Row label="サイズ">
          <NumberField
            value={doc.size}
            min={1}
            unit="px"
            onChange={(v, d) => editLayer("文字の大きさ", layer.id, (l) => void (l.kind.type === "text" && (l.kind.document.size = v)), d)}
          />
        </Row>
        <Row label="フォント">
          <FontPicker
            value={doc.font_family}
            text={doc.text}
            onChange={(v) => editLayer("フォント", layer.id, (l) => void (l.kind.type === "text" && (l.kind.document.font_family = v)))}
          />
        </Row>
        <Row label="塗り">
          <input
            type="color"
            className="h-8 w-12 rounded-3xl"
            value={toHex(doc.fill ?? [1, 1, 1, 1])}
            onChange={(e) =>
              editLayer("文字の色", layer.id, (l) => {
                if (l.kind.type === "text") l.kind.document.fill = [...fromHex(e.target.value), l.kind.document.fill?.[3] ?? 1];
              }, true)
            }
          />
        </Row>
      </Section>
    );
  }
  if (k.type === "solid" || k.type === "light") {
    return (
      <Section title={k.type === "solid" ? "平面" : "ライト"}>
        <Row label="色">
          <input
            type="color"
            className="h-8 w-12 rounded-3xl"
            value={toHex(k.color)}
            onChange={(e) =>
              editLayer("色", layer.id, (l) => void ((l.kind.type === "solid" || l.kind.type === "light") && (l.kind.color = fromHex(e.target.value))), true)
            }
          />
        </Row>
      </Section>
    );
  }
  return null;
}

// ---------- エフェクト ----------

// プラグインを読み込むとエフェクトが増えるので、プラグインの世代ごとに取り直す
let catalogCache: { gen: number; list: Promise<EffectInfo[]> } | null = null;
const catalog = (gen: number) => {
  if (catalogCache?.gen !== gen) catalogCache = { gen, list: api.effectCatalog() };
  return catalogCache.list;
};

function EffectsSection({ comp, layer, props }: { comp: Composition; layer: Layer; props: PropInfo[] }) {
  const [infos, setInfos] = useState<EffectInfo[]>([]);
  const [picking, setPicking] = useState(false);
  const gen = useUi((s) => s.pluginGen);
  useEffect(() => void catalog(gen).then(setInfos), [gen]);
  const effects = layer.effects ?? [];
  const add = async (kind: string) => {
    try {
      const fx = await api.makeEffect(kind, comp.id, layer.id);
      editLayer("エフェクトを追加", layer.id, (l) => void (l.effects = [...(l.effects ?? []), fx]));
    } catch (e) {
      set({ status: String(e) });
    }
  };
  return (
    <Section
      title="エフェクト"
      actions={
        <Popover open={picking} onOpenChange={setPicking}>
          <PopoverTrigger render={<Button variant="ghost" size="icon-sm" aria-label="エフェクトを追加" className="text-muted-foreground" />}>
            <Icon name="plus" />
          </PopoverTrigger>
          <PopoverContent align="end" className="w-72 p-0">
            <Command>
              <CommandInput placeholder="エフェクトを検索" onKeyDown={(e) => e.stopPropagation()} />
              <CommandList className="max-h-80">
                <CommandEmpty>一致するエフェクトがありません</CommandEmpty>
                {infos.map((i) => (
                  <CommandItem key={i.kind} value={`${i.label} ${i.category} ${i.kind}`} onSelect={() => (setPicking(false), void add(i.kind))}>
                    <span className="flex-1">{i.label}</span>
                    <span className="text-xs text-muted-foreground">{i.category}</span>
                  </CommandItem>
                ))}
              </CommandList>
            </Command>
          </PopoverContent>
        </Popover>
      }
    >
      {effects.length === 0 && <p className="text-xs text-muted-foreground/70">＋ からエフェクトを追加できます。</p>}
      {effects.map((fx, i) => (
        <EffectCard key={i} layer={layer} fx={fx} index={i} info={infos.find((x) => x.kind === fx.kind)} props={props} />
      ))}
    </Section>
  );
}

/**
 * エフェクト 1 つ。数値・位置・色のパラメータはプロパティの行（◆ でキーフレーム、= でエクスプレッション）で、
 * それ以外（スイッチ・選択肢・レイヤー・文字）はそのまま編集する。
 */
function EffectCard(props: { layer: Layer; fx: EffectInstance; index: number; info?: EffectInfo; props: PropInfo[] }) {
  const { layer, fx, index, info } = props;
  const [open, setOpen] = useState(true);
  const edit = (label: string, f: (fx: EffectInstance) => void) =>
    editLayer(label, layer.id, (l) => {
      const e = l.effects?.[index];
      if (!e) return false;
      f(e);
    });
  return (
    <div className={`flex flex-col gap-1.5 rounded-3xl bg-muted/50 p-2 ${fx.enabled === false ? "opacity-60" : ""}`}>
      <div className="flex items-center gap-1.5 pl-1">
        <button type="button" className={`twirl-btn ${open ? "open" : ""}`} onClick={() => setOpen((o) => !o)} aria-label="開く／閉じる">
          <Icon name="twirl" size={12} />
        </button>
        <Checkbox checked={fx.enabled !== false} onCheckedChange={(on) => edit("エフェクトの有効", (f) => void (f.enabled = on))} />
        <span className="flex-1 truncate text-sm font-medium">{fx.name}</span>
        {!info && <span className="text-xs text-amber-400">プラグインがありません</span>}
        <IconButton
          icon="up"
          title="上へ"
          size="icon-xs"
          disabled={index === 0}
          onClick={() =>
            editLayer("エフェクトの順番", layer.id, (l) => {
              const e = l.effects!;
              [e[index - 1], e[index]] = [e[index], e[index - 1]];
            })
          }
        />
        <IconButton icon="close" title="外す" size="icon-xs" onClick={() => editLayer("エフェクトを削除", layer.id, (l) => void l.effects!.splice(index, 1))} />
      </div>
      {open &&
        info?.params.map((p) => {
          const v = fx.params[p.name] as ParamValue | undefined;
          if (!v) return null;
          if (v.t === "scalar" || v.t === "point" || v.t === "color") {
            const prop = props.props.find((x) => x.path === `fx.${index}.${p.name}`);
            return prop ? <PropRow key={p.name} layer={layer} prop={prop} label={p.label} /> : null;
          }
          let control: React.ReactNode = <span className="text-muted-foreground">—</span>;
          if (v.t === "bool") {
            control = <Checkbox checked={v.v} onCheckedChange={(on) => edit(p.label, (f) => void ((f.params[p.name] as { v: boolean }).v = on))} />;
          } else if (v.t === "choice") {
            control = (
              <Choice
                value={String(v.v)}
                options={(p.choices ?? []).map((c, i): [string, string] => [String(i), c])}
                onChange={(c) => edit(p.label, (f) => void ((f.params[p.name] as { v: number }).v = Number(c)))}
              />
            );
          } else if (v.t === "text") {
            control = <TextField value={v.v} onChange={(s) => edit(p.label, (f) => void ((f.params[p.name] as { v: string }).v = s))} />;
          } else if (v.t === "layer") {
            const comp = currentComp()!;
            control = (
              <Choice
                value={v.v ?? NONE}
                options={[[NONE, "なし"], ...comp.layers.map((l): [string, string] => [l.id, l.name])]}
                onChange={(id) => edit(p.label, (f) => void ((f.params[p.name] as { v: string | null }).v = id === NONE ? null : id))}
              />
            );
          }
          return (
            <div key={p.name} className="pl-7">
              <Row label={p.label}>{control}</Row>
            </div>
          );
        })}
    </div>
  );
}

// ---------- 素材 ----------

function FootageInspector({ id }: { id: string }) {
  const f = useUi((s) => s.project?.footage.find((x) => x.id === id));
  if (!f) return null;
  const length = f.duration ?? f.audio?.duration;
  return (
    <>
      <div className="flex flex-col gap-1 px-4 pt-3.5 pb-3">
        <div className="text-[15px] font-semibold">{f.name}</div>
        <div className="text-xs break-all text-muted-foreground">{f.path}</div>
      </div>
      <Section title="素材">
        {f.width > 0 && <Row label="大きさ">{`${f.width} × ${f.height}`}</Row>}
        {f.frame_rate && <Row label="フレームレート">{(f.frame_rate.num / f.frame_rate.den).toFixed(2)} fps</Row>}
        {length !== undefined && <Row label="長さ">{seconds(length).toFixed(2)} 秒</Row>}
        <Button
          variant="secondary"
          onClick={async () => {
            const s = get();
            try {
              const layer = await api.makeLayer(`footage:${f.id}`, s.compId, s.frame);
              editComp("レイヤー追加", (c) => void c.layers.unshift(layer));
              set({ selected: layer.id, selectedFootage: null });
            } catch (e) {
              set({ status: String(e) });
            }
          }}
        >
          現在のコンポに追加
        </Button>
      </Section>
    </>
  );
}

// ---------- コンポ ----------

function CompInspector({ comp }: { comp: Composition }) {
  const rate = FRAME_RATES.find((r) => r.fr.num === comp.frame_rate.num && r.fr.den === comp.frame_rate.den);
  const mb = comp.motion_blur ?? { enabled: false, shutter_angle: 180, shutter_phase: -90, samples: 16 };
  return (
    <>
      <div className="flex flex-col gap-2 px-4 pt-3.5 pb-3">
        <TextField className="h-9 text-[15px] font-semibold" value={comp.name} onChange={(v) => editComp("コンポ名", (c) => void (c.name = v))} />
        <div className="text-xs text-muted-foreground">
          {comp.layers.length} レイヤー・レイヤーを選ぶと、その設定をここで編集できます
        </div>
      </div>
      <Section title="コンポジション">
        <Row label="大きさ">
          <NumberField value={comp.width} digits={0} min={1} max={16384} onChange={(v, d) => editComp("コンポの大きさ", (c) => void (c.width = Math.round(v)), d)} />
          <NumberField value={comp.height} digits={0} min={1} max={16384} onChange={(v, d) => editComp("コンポの大きさ", (c) => void (c.height = Math.round(v)), d)} />
        </Row>
        <Row label="フレームレート">
          <Choice
            value={rate?.label ?? "custom"}
            options={[
              ...(rate ? [] : [["custom", `${(comp.frame_rate.num / comp.frame_rate.den).toFixed(3)} fps`] as [string, string]]),
              ...FRAME_RATES.map((r): [string, string] => [r.label, `${r.label} fps`]),
            ]}
            onChange={(v) => {
              const r = FRAME_RATES.find((x) => x.label === v);
              if (r) editComp("フレームレート", (c) => void (c.frame_rate = r.fr));
            }}
          />
        </Row>
        <Row label="長さ">
          <NumberField value={seconds(comp.duration)} digits={2} step={0.05} min={0.04} unit="秒" onChange={(v, d) => editComp("コンポの長さ", (c) => void (c.duration = fromSeconds(v)), d)} />
        </Row>
        <Row label="背景色">
          <input type="color" value={toHex(comp.background ?? [0, 0, 0])} onChange={(e) => editComp("背景色", (c) => void (c.background = fromHex(e.target.value)), true)} />
        </Row>
      </Section>
      <Section title="モーションブラー" defaultOpen={mb.enabled}>
        <Row label="有効">
          <Checkbox checked={mb.enabled} onCheckedChange={(on) => editComp("モーションブラー", (c) => void (c.motion_blur = { ...mb, enabled: on }))} />
        </Row>
        <Row label="シャッター角度">
          <NumberField value={mb.shutter_angle} min={0} max={720} unit="°" onChange={(v, d) => editComp("シャッター角度", (c) => void (c.motion_blur = { ...mb, shutter_angle: v }), d)} />
        </Row>
        <Row label="サンプル数">
          <NumberField value={mb.samples} digits={0} min={2} max={64} onChange={(v, d) => editComp("サンプル数", (c) => void (c.motion_blur = { ...mb, samples: Math.round(v) }), d)} />
        </Row>
      </Section>
      <CompExtras comp={comp} />
      <Section title="ファイル" defaultOpen={false}>
        <Button variant="secondary" onClick={() => void run(() => api.save(false), "保存しました")}>
          保存
        </Button>
      </Section>
    </>
  );
}
