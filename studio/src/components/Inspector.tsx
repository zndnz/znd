// インスペクタ（右）。選んだものの設定をすべてここに出す。
// レイヤー → その全設定、素材 → その情報、どちらも選んでいなければ現在のコンポの設定。

import { useEffect, useMemo, useState } from "react";
import { api, type Evaluated } from "../api";
import {
  BLEND_MODES,
  type Composition,
  type EffectInfo,
  type EffectInstance,
  FRAME_RATES,
  frameToTime,
  fromSeconds,
  type Layer,
  type ParamValue,
  type Property,
  seconds,
  timecode,
  timeToFrame,
} from "../model";
import { getProp, isAnimated, type PropPath, setValue, THREE_D_PROPS, toggleAnimation, toggleKeyAt, TRANSFORM_PROPS } from "../ops";
import { currentComp, editComp, editLayer, get, run, selectedLayer, set, useUi } from "../store";
import { IconButton, MenuItem, NumberField, Popover, Row, Section, TextField } from "./controls";
import { Icon } from "./icons";
import { layerActions } from "./layerActions";

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

/** 今の時刻のトランスフォームの値（キーフレームを補間したもの）。 */
function useEvaluated(comp: string, layer: Layer, frame: number): Evaluated | null {
  const revision = useUi((s) => s.snap?.revision ?? 0);
  const [v, setV] = useState<Evaluated | null>(null);
  const animated = [...TRANSFORM_PROPS, ...THREE_D_PROPS].some((p) => isAnimated(getProp(layer, p.path)));
  useEffect(() => {
    if (!animated) return setV(null);
    let alive = true;
    void api.evalLayer(comp, layer.id, frame).then((r) => alive && setV(r));
    return () => {
      alive = false;
    };
  }, [comp, layer.id, frame, revision, animated, layer]);
  return v;
}

function LayerInspector({ comp, layer }: { comp: Composition; layer: Layer }) {
  const frame = useUi((s) => s.frame);
  const evaluated = useEvaluated(comp.id, layer, frame);
  const a = layerActions(layer.id);
  const fr = comp.frame_rate;
  const t = frameToTime(fr, frame);
  const inF = timeToFrame(fr, layer.in_point);
  const outF = timeToFrame(fr, layer.out_point);
  const others = comp.layers.filter((l) => l.id !== layer.id);
  const props = [...TRANSFORM_PROPS, ...(layer.three_d ? THREE_D_PROPS : [])];

  return (
    <>
      <div className="inspector-head">
        <TextField value={layer.name} onChange={(v) => editLayer("名前", layer.id, (l) => void (l.name = v))} />
        <div className="switches">
          <IconButton icon={layer.visible === false ? "eyeOff" : "eye"} title="表示" on={layer.visible !== false} onClick={a.toggleVisible} />
          <IconButton icon="solo" title="ソロ" on={!!layer.solo} onClick={a.toggleSolo} />
          <IconButton icon={layer.locked ? "lock" : "unlock"} title="ロック" on={!!layer.locked} onClick={a.toggleLock} />
          <IconButton icon="threeD" title="3D レイヤー" on={!!layer.three_d} onClick={a.toggle3d} />
          <IconButton icon="blur" title="モーションブラー" on={!!layer.motion_blur} onClick={a.toggleMotionBlur} />
          <span className="grow" />
          <IconButton icon="trash" title="削除" onClick={a.remove} />
        </div>
        <div className="timing" title="イン点 – アウト点（長さ）">
          {timecode(fr, inF)} – {timecode(fr, outF)} <span className="dim">（{timecode(fr, outF - inF)}）</span>
        </div>
      </div>

      <fieldset disabled={!!layer.locked} className="plain">
        <KindSection layer={layer} />

        <Section title="トランスフォーム">
          {props.map((p) => {
            const prop = getProp(layer, p.path);
            const shown = (evaluated?.[p.path as keyof Evaluated] ?? prop?.value ?? 0) as number | [number, number];
            const animated = isAnimated(prop);
            const keyHere = !!prop?.keyframes?.some((k) => k.time === t);
            const change = (v: number | [number, number], dragging: boolean) =>
              editLayer(p.label, layer.id, (l) => setValue(l, p.path as PropPath, t, v), dragging);
            return (
              <div className="prop" key={p.path}>
                <button
                  type="button"
                  className={`key-toggle ${animated ? "animated" : ""} ${keyHere ? "here" : ""}`}
                  title={animated ? "この時刻にキーフレームを打つ／外す" : "アニメーションを始める（今の値でキーフレームを打つ）"}
                  onClick={() =>
                    editLayer("キーフレーム", layer.id, (l) =>
                      animated ? toggleKeyAt(l, p.path as PropPath, t, shown) : toggleAnimation(l, p.path as PropPath, t, shown),
                    )
                  }
                  onContextMenu={(e) => {
                    e.preventDefault();
                    if (animated) editLayer("アニメーションを止める", layer.id, (l) => toggleAnimation(l, p.path as PropPath, t, shown));
                  }}
                >
                  <Icon name="diamond" size={12} />
                </button>
                <span className="prop-name">{p.label}</span>
                <span className="prop-values">
                  {Array.isArray(shown) ? (
                    <>
                      <NumberField value={shown[0]} unit={p.unit} onChange={(v, d) => change([v, shown[1]], d)} />
                      <NumberField value={shown[1]} unit={p.unit} onChange={(v, d) => change([shown[0], v], d)} />
                    </>
                  ) : (
                    <NumberField
                      value={shown}
                      unit={p.unit}
                      min={p.path === "opacity" ? 0 : undefined}
                      max={p.path === "opacity" ? 100 : undefined}
                      step={p.path === "opacity" ? 0.5 : 1}
                      onChange={(v, d) => change(v, d)}
                    />
                  )}
                </span>
              </div>
            );
          })}
          <p className="hint">◆ を押すとアニメーションが始まります。右クリックでアニメーションを止めます。</p>
        </Section>

        <Section title="合成">
          <Row label="描画モード">
            <select value={layer.blend_mode ?? "normal"} onChange={(e) => editLayer("描画モード", layer.id, (l) => void (l.blend_mode = e.target.value))}>
              {BLEND_MODES.map(([v, label]) => (
                <option key={v} value={v}>
                  {label}
                </option>
              ))}
            </select>
          </Row>
          <Row label="親">
            <select value={layer.parent ?? ""} onChange={(e) => editLayer("親", layer.id, (l) => void (l.parent = e.target.value || null))}>
              <option value="">なし</option>
              {others.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
          </Row>
          <Row label="トラックマット">
            <select
              value={(layer.track_matte as { layer: string } | null | undefined)?.layer ?? ""}
              onChange={(e) =>
                editLayer("トラックマット", layer.id, (l) => {
                  const cur = l.track_matte as { layer: string; mode: string } | null | undefined;
                  l.track_matte = e.target.value ? { layer: e.target.value, mode: cur?.mode ?? "alpha" } : null;
                })
              }
            >
              <option value="">なし</option>
              {others.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
          </Row>
        </Section>

        <EffectsSection comp={comp} layer={layer} />
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
          <TextField
            value={doc.font_family}
            placeholder="既定のフォント"
            onChange={(v) => editLayer("フォント", layer.id, (l) => void (l.kind.type === "text" && (l.kind.document.font_family = v)))}
          />
        </Row>
        <Row label="塗り">
          <input
            type="color"
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

let catalogCache: Promise<EffectInfo[]> | null = null;
const catalog = () => (catalogCache ??= api.effectCatalog());

function EffectsSection({ comp, layer }: { comp: Composition; layer: Layer }) {
  const [infos, setInfos] = useState<EffectInfo[]>([]);
  const [query, setQuery] = useState("");
  useEffect(() => void catalog().then(setInfos), []);
  const effects = layer.effects ?? [];
  const found = useMemo(() => {
    const q = query.trim().toLowerCase();
    return infos.filter((i) => !q || `${i.label} ${i.category}`.toLowerCase().includes(q));
  }, [infos, query]);
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
        <Popover
          align="end"
          trigger={(open, toggle) => <IconButton icon="plus" title="エフェクトを追加" on={open} onClick={toggle} />}
        >
          {(close) => (
            <div className="fx-picker">
              <input autoFocus className="text-field" placeholder="エフェクトを検索" value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.stopPropagation()} />
              <div className="fx-list">
                {found.map((i) => (
                  <MenuItem key={i.kind} label={i.label} hint={i.category} onClick={() => (close(), setQuery(""), void add(i.kind))} />
                ))}
              </div>
            </div>
          )}
        </Popover>
      }
    >
      {effects.length === 0 && <p className="hint">＋ からエフェクトを追加できます。</p>}
      {effects.map((fx, i) => (
        <EffectCard key={i} layer={layer} fx={fx} index={i} info={infos.find((x) => x.kind === fx.kind)} />
      ))}
    </Section>
  );
}

function EffectCard({ layer, fx, index, info }: { layer: Layer; fx: EffectInstance; index: number; info?: EffectInfo }) {
  const [open, setOpen] = useState(true);
  const t = frameToTime(currentComp()!.frame_rate, get().frame);
  const edit = (label: string, f: (fx: EffectInstance) => void, coalesce = false) =>
    editLayer(label, layer.id, (l) => {
      const e = l.effects?.[index];
      if (!e) return false;
      f(e);
    }, coalesce);
  const setProp = <V,>(p: Property<V>, v: V) => {
    if (p.keyframes?.length) {
      const k = p.keyframes.find((k) => k.time === t);
      if (k) k.value = v;
      else {
        p.keyframes.push({ time: t, value: v, interp_in: "linear", interp_out: "linear" });
        p.keyframes.sort((a, b) => a.time - b.time);
      }
    } else p.value = v;
  };
  return (
    <div className={`fx ${fx.enabled === false ? "off" : ""}`}>
      <div className="fx-head">
        <button type="button" className={`twirl-btn ${open ? "open" : ""}`} onClick={() => setOpen((o) => !o)}>
          <Icon name="twirl" size={10} />
        </button>
        <input type="checkbox" checked={fx.enabled !== false} onChange={(e) => edit("エフェクトの有効", (f) => void (f.enabled = e.target.checked))} />
        <span className="fx-name">{fx.name}</span>
        {!info && <span className="warn">プラグインがありません</span>}
        <IconButton
          icon="up"
          title="上へ"
          disabled={index === 0}
          onClick={() =>
            editLayer("エフェクトの順番", layer.id, (l) => {
              const e = l.effects!;
              [e[index - 1], e[index]] = [e[index], e[index - 1]];
            })
          }
        />
        <IconButton icon="close" title="外す" onClick={() => editLayer("エフェクトを削除", layer.id, (l) => void l.effects!.splice(index, 1))} />
      </div>
      {open &&
        info?.params.map((p) => {
          const v = fx.params[p.name] as ParamValue | undefined;
          if (!v) return null;
          let control: React.ReactNode = <span className="dim">—</span>;
          if (v.t === "scalar") {
            const shown = v.v.keyframes?.find((k) => k.time === t)?.value ?? v.v.value;
            control = (
              <NumberField
                value={shown}
                min={p.min}
                max={p.max}
                step={p.max !== undefined && p.min !== undefined ? Math.max(0.01, (p.max - p.min) / 400) : 1}
                digits={2}
                onChange={(n, d) => edit(p.label, (f) => setProp((f.params[p.name] as { v: Property<number> }).v, n), d)}
              />
            );
          } else if (v.t === "point") {
            const s = v.v.value;
            control = (
              <>
                <NumberField value={s[0]} onChange={(n, d) => edit(p.label, (f) => setProp((f.params[p.name] as { v: Property<[number, number]> }).v, [n, s[1]]), d)} />
                <NumberField value={s[1]} onChange={(n, d) => edit(p.label, (f) => setProp((f.params[p.name] as { v: Property<[number, number]> }).v, [s[0], n]), d)} />
              </>
            );
          } else if (v.t === "color") {
            const c = v.v.value;
            control = (
              <input
                type="color"
                value={toHex(c)}
                onChange={(e) => edit(p.label, (f) => setProp((f.params[p.name] as { v: Property<number[]> }).v, [...fromHex(e.target.value), c[3]]), true)}
              />
            );
          } else if (v.t === "bool") {
            control = <input type="checkbox" checked={v.v} onChange={(e) => edit(p.label, (f) => void ((f.params[p.name] as { v: boolean }).v = e.target.checked))} />;
          } else if (v.t === "choice") {
            control = (
              <select value={v.v} onChange={(e) => edit(p.label, (f) => void ((f.params[p.name] as { v: number }).v = Number(e.target.value)))}>
                {(p.choices ?? []).map((c, i) => (
                  <option key={c} value={i}>
                    {c}
                  </option>
                ))}
              </select>
            );
          } else if (v.t === "text") {
            control = <TextField value={v.v} onChange={(s) => edit(p.label, (f) => void ((f.params[p.name] as { v: string }).v = s))} />;
          } else if (v.t === "layer") {
            const comp = currentComp()!;
            control = (
              <select value={v.v ?? ""} onChange={(e) => edit(p.label, (f) => void ((f.params[p.name] as { v: string | null }).v = e.target.value || null))}>
                <option value="">なし</option>
                {comp.layers.map((l) => (
                  <option key={l.id} value={l.id}>
                    {l.name}
                  </option>
                ))}
              </select>
            );
          }
          return (
            <Row key={p.name} label={p.label}>
              {control}
            </Row>
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
      <div className="inspector-head">
        <div className="title">{f.name}</div>
        <div className="timing dim">{f.path}</div>
      </div>
      <Section title="素材">
        {f.width > 0 && <Row label="大きさ">{`${f.width} × ${f.height}`}</Row>}
        {f.frame_rate && <Row label="フレームレート">{(f.frame_rate.num / f.frame_rate.den).toFixed(2)} fps</Row>}
        {length !== undefined && <Row label="長さ">{seconds(length).toFixed(2)} 秒</Row>}
        <button
          type="button"
          className="wide-btn"
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
        </button>
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
      <div className="inspector-head">
        <TextField value={comp.name} onChange={(v) => editComp("コンポ名", (c) => void (c.name = v))} />
        <div className="timing dim">
          {comp.layers.length} レイヤー・レイヤーを選ぶと、その設定をここで編集できます
        </div>
      </div>
      <Section title="コンポジション">
        <Row label="大きさ">
          <NumberField value={comp.width} digits={0} min={1} max={16384} onChange={(v, d) => editComp("コンポの大きさ", (c) => void (c.width = Math.round(v)), d)} />
          <NumberField value={comp.height} digits={0} min={1} max={16384} onChange={(v, d) => editComp("コンポの大きさ", (c) => void (c.height = Math.round(v)), d)} />
        </Row>
        <Row label="フレームレート">
          <select
            value={rate?.label ?? ""}
            onChange={(e) => {
              const r = FRAME_RATES.find((x) => x.label === e.target.value);
              if (r) editComp("フレームレート", (c) => void (c.frame_rate = r.fr));
            }}
          >
            {!rate && <option value="">{(comp.frame_rate.num / comp.frame_rate.den).toFixed(3)}</option>}
            {FRAME_RATES.map((r) => (
              <option key={r.label} value={r.label}>
                {r.label} fps
              </option>
            ))}
          </select>
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
          <input type="checkbox" checked={mb.enabled} onChange={(e) => editComp("モーションブラー", (c) => void (c.motion_blur = { ...mb, enabled: e.target.checked }))} />
        </Row>
        <Row label="シャッター角度">
          <NumberField value={mb.shutter_angle} min={0} max={720} unit="°" onChange={(v, d) => editComp("シャッター角度", (c) => void (c.motion_blur = { ...mb, shutter_angle: v }), d)} />
        </Row>
        <Row label="サンプル数">
          <NumberField value={mb.samples} digits={0} min={2} max={64} onChange={(v, d) => editComp("サンプル数", (c) => void (c.motion_blur = { ...mb, samples: Math.round(v) }), d)} />
        </Row>
      </Section>
      <Section title="ファイル" defaultOpen={false}>
        <button type="button" className="wide-btn" onClick={() => void run(() => api.save(false), "保存しました")}>
          保存
        </button>
      </Section>
    </>
  );
}
