// 画面全体。左にプロジェクト、中央にビューアとタイムライン、右にインスペクタ。
// メニューバーの項目とキー操作もここで受ける（Rust からは項目の ID が届くだけで、何をするかはここで決める）。

import { useEffect } from "react";
import { api } from "./api";
import { Inspector } from "./components/Inspector";
import { IconButton } from "./components/controls";
import { layerActions } from "./components/layerActions";
import { newComposition, ProjectList } from "./components/ProjectList";
import { deleteSelectedKey, Timeline } from "./components/Timeline";
import { createLayer, Viewer } from "./components/Viewer";
import { fps } from "./model";
import { adopt, currentComp, get, run, set, setFrame, totalFrames, useUi } from "./store";

function onMenu(id: string) {
  const s = get();
  if (id.startsWith("layer:")) return void createLayer(id.slice(6));
  switch (id) {
    case "new_demo":
      return void run(api.newDemo, "デモプロジェクトを開きました");
    case "open":
      return void run(() => api.open(), "開きました");
    case "save":
      return void run(() => api.save(false), "保存しました");
    case "save_as":
      return void run(() => api.save(true), "保存しました");
    case "import":
      return void run(() => api.import(s.compId, s.frame, false), "素材を読み込みました");
    case "import_sequence":
      return void run(() => api.import(s.compId, s.frame, true), "連番を読み込みました");
    case "new_comp":
      return newComposition();
    case "undo":
      if (isTyping()) return void document.execCommand("undo");
      return void run(api.undo);
    case "redo":
      if (isTyping()) return void document.execCommand("redo");
      return void run(api.redo);
    case "duplicate":
      return s.selected && layerActions(s.selected).duplicate();
    case "bring_forward":
      return s.selected && layerActions(s.selected).reorder(-1);
    case "send_backward":
      return s.selected && layerActions(s.selected).reorder(1);
    case "toggle_project":
      return set({ showProject: !s.showProject });
    case "toggle_inspector":
      return set({ showInspector: !s.showInspector });
    case "zoom_fit":
      return set({ viewZoom: 0, viewPan: [0, 0] });
    case "timeline_fit":
      return set({ tlZoom: 1, tlStart: 0 });
  }
}

const isTyping = () => {
  const el = document.activeElement;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT");
};

function onKey(e: KeyboardEvent) {
  if (isTyping() || e.metaKey || e.ctrlKey) return;
  const s = get();
  const step = e.shiftKey ? 10 : 1;
  switch (e.key) {
    case " ":
      e.preventDefault();
      return set({ playing: !s.playing });
    case "ArrowLeft":
      return setFrame(s.frame - step);
    case "ArrowRight":
      return setFrame(s.frame + step);
    case "Home":
      return setFrame(0);
    case "End":
      return setFrame(totalFrames() - 1);
    case "v":
      return set({ tool: "select" });
    case "h":
      return set({ tool: "hand" });
    case "t":
      return void createLayer("text");
    case "b":
      return set((st) => ({ workArea: [s.frame, st.workArea?.[1] ?? totalFrames()] }));
    case "n":
      return set((st) => ({ workArea: [st.workArea?.[0] ?? 0, Math.max(s.frame + 1, (st.workArea?.[0] ?? 0) + 1)] }));
    case "Escape":
      return set({ selected: null, selectedFootage: null, selectedKey: null });
    case "Backspace":
    case "Delete":
      if (deleteSelectedKey()) return;
      if (s.selected) layerActions(s.selected).remove();
      return;
  }
}

/** 再生中は、経過時間からフレームを決める（ワークエリアがあればその中を繰り返す）。 */
function usePlayback() {
  const playing = useUi((s) => s.playing);
  useEffect(() => {
    if (!playing) return;
    const comp = currentComp();
    if (!comp) return;
    const rate = fps(comp.frame_rate);
    const t0 = performance.now();
    const f0 = get().frame;
    let raf = 0;
    const tick = (now: number) => {
      const total = totalFrames();
      const [a, b] = get().workArea ?? [0, total];
      const len = Math.max(1, b - a);
      let f = f0 + Math.floor(((now - t0) / 1000) * rate);
      if (f >= b) f = a + ((f - a) % len);
      if (get().frame !== f) set({ frame: f });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]);
}

export function App() {
  const ready = useUi((s) => !!s.project);
  const showProject = useUi((s) => s.showProject);
  const showInspector = useUi((s) => s.showInspector);
  const title = useUi((s) => (s.snap?.path ? s.snap.path.split(/[\\/]/).pop()! : "未保存"));
  const dirty = useUi((s) => !!s.snap?.dirty);
  const status = useUi((s) => s.status);
  usePlayback();

  useEffect(() => {
    void api.snapshot().then(adopt);
    const un = api.onMenu(onMenu);
    window.addEventListener("keydown", onKey);
    return () => {
      void un.then((u) => u());
      window.removeEventListener("keydown", onKey);
    };
  }, []);
  // メッセージは少ししたら消す
  useEffect(() => {
    if (!status) return;
    const id = setTimeout(() => set({ status: "" }), 4000);
    return () => clearTimeout(id);
  }, [status]);

  if (!ready) return <div className="app loading" />;
  return (
    <div className={`app ${showProject ? "" : "no-left"} ${showInspector ? "" : "no-right"}`}>
      <header className="titlebar" data-tauri-drag-region>
        <span className="titlebar-left" data-tauri-drag-region>
          <IconButton icon="sidebarL" title="プロジェクト (⌘1)" on={showProject} onClick={() => set({ showProject: !showProject })} />
        </span>
        <span className="title" data-tauri-drag-region>
          {title}
          {dirty && <span className="dirty" title="保存していない変更があります" />}
        </span>
        <span className="titlebar-right" data-tauri-drag-region>
          {status && <span className="status">{status}</span>}
          <IconButton icon="sidebarR" title="インスペクタ (⌘2)" on={showInspector} onClick={() => set({ showInspector: !showInspector })} />
        </span>
      </header>
      {showProject && <ProjectList />}
      <main className="center">
        <Viewer />
        <Timeline />
      </main>
      {showInspector && <Inspector />}
    </div>
  );
}
