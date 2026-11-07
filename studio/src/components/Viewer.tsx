// ビューア。黒い面の中央にコンポを出し、左上に表示の設定、下の中央に浮かぶツールパレットを置く。

import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { frameToTime } from "../model";
import { getProp, setValue } from "../ops";
import { currentComp, editComp, editLayer, get, run, selectedLayer, set, useUi } from "../store";
import { IconButton, MenuItem, Popover } from "./controls";
import { Icon } from "./icons";

const ZOOMS: [number, string][] = [
  [0, "全体表示"],
  [0.25, "25%"],
  [0.5, "50%"],
  [1, "100%"],
  [2, "200%"],
];
const RESOLUTIONS: [number, string][] = [
  [1, "フル画質"],
  [2, "1/2 画質"],
  [4, "1/4 画質"],
];

export function Viewer() {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [box, setBox] = useState({ w: 800, h: 450 });
  const [rendering, setRendering] = useState(false);
  const [guides, setGuides] = useState(false);
  const comp = useUi(currentComp);
  const revision = useUi((s) => s.snap?.revision ?? 0);
  const frame = useUi((s) => s.frame);
  const compId = useUi((s) => s.compId);
  const downsample = useUi((s) => s.downsample);
  const zoom = useUi((s) => s.viewZoom);
  const pan = useUi((s) => s.viewPan);
  const tool = useUi((s) => s.tool);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 表示したいフレームを Rust に頼み、できていれば描く。できたら "frame-ready" が来るので頼み直す
  useEffect(() => {
    let alive = true;
    const draw = async () => {
      const f = await api.frame(compId, frame, downsample);
      if (!alive) return;
      setRendering(!f);
      const c = canvas.current;
      if (!f || !c) return;
      if (c.width !== f.width || c.height !== f.height) {
        c.width = f.width;
        c.height = f.height;
      }
      c.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(f.rgba), f.width, f.height), 0, 0);
    };
    void draw();
    const un = api.onFrameReady(() => void draw());
    return () => {
      alive = false;
      void un.then((u) => u());
    };
  }, [compId, frame, downsample, revision]);

  if (!comp) return <div className="viewer" ref={wrap} />;
  const fit = Math.min((box.w - 80) / comp.width, (box.h - 140) / comp.height);
  const scale = zoom > 0 ? zoom : Math.max(0.02, fit);
  const w = comp.width * scale;
  const h = comp.height * scale;
  const left = (box.w - w) / 2 + pan[0];
  const top = (box.h - h) / 2 - 24 + pan[1];

  // 選択ツールでドラッグ: 選んだレイヤーを動かす。手のひらツール（または中ボタン・Alt）でドラッグ: 画面を動かす
  const onPointerDown = (e: React.PointerEvent) => {
    const panning = tool === "hand" || e.button === 1 || e.altKey;
    const layer = selectedLayer();
    if (!panning && (!layer || layer.locked || e.button !== 0)) return;
    const start = { x: e.clientX, y: e.clientY, pan: get().viewPan };
    const t = frameToTime(comp.frame_rate, get().frame);
    // 動かし始めの位置（キーフレームを補間した値）。届くまでの動きは届いてから反映する
    let base: [number, number] | null = null;
    let last: [number, number] = [0, 0];
    const apply = () => {
      if (!layer || !base) return;
      const p: [number, number] = [Math.round((base[0] + last[0] / scale) * 10) / 10, Math.round((base[1] + last[1] / scale) * 10) / 10];
      editLayer("移動", layer.id, (l) => setValue(l, "position", t, p), true);
    };
    if (!panning && layer) {
      void api.evalLayer(comp.id, layer.id, get().frame).then((v) => {
        base = v?.position ?? (getProp(layer, "position")?.value as [number, number]);
        apply();
      });
    }
    const move = (ev: PointerEvent) => {
      last = [ev.clientX - start.x, ev.clientY - start.y];
      if (panning) set({ viewPan: [start.pan[0] + last[0], start.pan[1] + last[1]] });
      else apply();
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const onWheel = (e: React.WheelEvent) => {
    if (e.ctrlKey || e.metaKey) {
      // ピンチ・⌘ + ホイールで拡大縮小
      const z = Math.max(0.05, Math.min(8, scale * Math.exp(-e.deltaY * 0.01)));
      set({ viewZoom: z });
    } else set((s) => ({ viewPan: [s.viewPan[0] - e.deltaX, s.viewPan[1] - e.deltaY] }));
  };

  return (
    <div className={`viewer tool-${tool}`} ref={wrap} onPointerDown={onPointerDown} onWheel={onWheel}>
      <div className="stage" style={{ left, top, width: w, height: h }}>
        <canvas ref={canvas} style={{ width: w, height: h }} />
        {guides && (
          <svg className="guides" viewBox="0 0 100 100" preserveAspectRatio="none">
            <rect x="5" y="5" width="90" height="90" />
            <rect x="10" y="10" width="80" height="80" />
          </svg>
        )}
      </div>

      <div className="float top-left pill">
        <Popover
          trigger={(open, toggle) => (
            <button type="button" className={`pill-btn ${open ? "on" : ""}`} onClick={toggle} title="表示の設定">
              {zoom > 0 ? `${Math.round(zoom * 100)}%` : "全体表示"}
              <span className="dim">・{RESOLUTIONS.find((r) => r[0] === downsample)?.[1]}</span>
              <Icon name="chevron" size={12} />
            </button>
          )}
        >
          {(close) => (
            <>
              <div className="menu-heading">拡大率</div>
              {ZOOMS.map(([z, label]) => (
                <MenuItem key={label} label={label} checked={zoom === z} onClick={() => (set({ viewZoom: z, viewPan: [0, 0] }), close())} />
              ))}
              <div className="menu-heading">画質</div>
              {RESOLUTIONS.map(([d, label]) => (
                <MenuItem key={label} label={label} checked={downsample === d} onClick={() => (set({ downsample: d }), close())} />
              ))}
              <div className="menu-sep" />
              <MenuItem label="セーフマージン" checked={guides} onClick={() => setGuides((g) => !g)} />
            </>
          )}
        </Popover>
        {rendering && <span className="pill-note">レンダリング中…</span>}
      </div>

      <ToolPalette />
    </div>
  );
}

const SHAPES: [string, string, string][] = [
  ["rect", "rect", "矩形"],
  ["ellipse", "ellipse", "楕円"],
  ["star", "star", "スター"],
];
const MORE: [string, string, string][] = [
  ["solid", "solid", "平面"],
  ["null", "null", "ヌル"],
  ["adjustment", "adjust", "調整レイヤー"],
  ["camera", "camera", "カメラ"],
  ["light_point", "light", "ライト（ポイント）"],
  ["light_spot", "light", "ライト（スポット）"],
  ["light_parallel", "light", "ライト（平行）"],
  ["light_ambient", "light", "ライト（アンビエント）"],
];

/** 新しいレイヤーを再生ヘッドの位置に作って選ぶ。 */
export async function createLayer(kind: string) {
  const s = get();
  try {
    const layer = await api.makeLayer(kind, s.compId, s.frame);
    editComp("レイヤー追加", (c) => void c.layers.unshift(layer));
    set({ selected: layer.id, selectedFootage: null });
  } catch (e) {
    set({ status: String(e) });
  }
}

function ToolPalette() {
  const tool = useUi((s) => s.tool);
  const [shape, setShape] = useState(0);
  return (
    <div className="float bottom-center palette">
      <IconButton icon="select" title="選択ツール (V)" on={tool === "select"} onClick={() => set({ tool: "select" })} />
      <IconButton icon="hand" title="手のひらツール (H)" on={tool === "hand"} onClick={() => set({ tool: "hand" })} />
      <span className="palette-sep" />
      <IconButton icon="text" title="テキスト (T)" onClick={() => void createLayer("text")} />
      <span className="split">
        <IconButton icon={SHAPES[shape][1]} title={`シェイプ（${SHAPES[shape][2]}）`} onClick={() => void createLayer(SHAPES[shape][0])} />
        <Popover
          placement="above"
          align="center"
          trigger={(_, toggle) => <IconButton icon="chevron" size={12} className="chev" title="シェイプの種類" onClick={toggle} />}
        >
          {(close) =>
            SHAPES.map(([kind, icon, label], i) => (
              <MenuItem key={kind} icon={icon} label={label} onClick={() => (setShape(i), close(), void createLayer(kind))} />
            ))
          }
        </Popover>
      </span>
      <IconButton
        icon="image"
        title="素材を読み込む (⌘I)"
        onClick={() => {
          const s = get();
          void run(() => api.import(s.compId, s.frame, false), "素材を読み込みました");
        }}
      />
      <Popover
        placement="above"
        align="center"
        trigger={(open, toggle) => <IconButton icon="more" title="ほかのレイヤー" on={open} onClick={toggle} />}
      >
        {(close) =>
          MORE.map(([kind, icon, label]) => <MenuItem key={kind} icon={icon} label={label} onClick={() => (close(), void createLayer(kind))} />)
        }
      </Popover>
    </div>
  );
}
