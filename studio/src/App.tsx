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
import { adopt, currentComp, get, keysOp, run, set, setFrame, totalFrames, useUi } from "./store";
import { applyRememberedEase } from "./components/Easing";
import { onPluginKey, onPluginMenu, PluginLog, PluginManager, PluginNotices, PluginPanels, PluginToolbar, refreshPlugins } from "./components/Plugins";
import { TooltipProvider } from "@/components/ui/tooltip";

function onMenu(id: string) {
  const s = get();
  if (id.startsWith("layer:")) return void createLayer(id.slice(6));
  if (onPluginMenu(id)) return;
  switch (id) {
    case "new":
      return void run(api.newProject, "新しいプロジェクトを作りました");
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
  if (isTyping()) return;
  if (onPluginKey(e)) return;
  if (e.metaKey || e.ctrlKey) return;
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
    case "e":
      return applyRememberedEase();
    case "F9":
      return void keysOp("イージーイーズ", { op: "kind", kind: "easy_ease" });
    case "Escape":
      return set({ selected: null, selectedFootage: null, selectedKeys: null });
    case "Backspace":
    case "Delete":
      if (deleteSelectedKey()) return;
      if (s.selected) layerActions(s.selected).remove();
      return;
  }
}

/**
 * 再生中のフレームを決める。音があれば Rust が鳴らしている位置に合わせ（映像が音に合わせて進む）、
 * 無ければ経過時間で進める。ワークエリアがあればその中を繰り返す。
 */
function usePlayback() {
  const playing = useUi((s) => s.playing);
  useEffect(() => {
    if (!playing) return;
    const comp = currentComp();
    if (!comp) return;
    const compId = comp.id;
    const rate = fps(comp.frame_rate);
    const total = totalFrames();
    const [a, b] = get().workArea ?? [0, total];
    // ワークエリアの外にいたら先頭から
    if (get().frame < a || get().frame >= b) set({ frame: a });
    let t0 = performance.now();
    let f0 = get().frame;
    let raf = 0;
    let asking = false;
    let stopped = false;
    void api.play(compId, f0, a, b);
    const tick = (now: number) => {
      const len = Math.max(1, b - a);
      let f = f0 + Math.floor(((now - t0) / 1000) * rate);
      if (f >= b) f = a + ((f - a) % len);
      if (get().frame !== f) set({ frame: f });
      // 音の位置を尋ね、返ってきたらそこを起点にする（前の問い合わせが返るまでは待たない）
      if (!asking) {
        asking = true;
        void api
          .playClock(compId, f)
          .then((af) => {
            if (stopped || af === null) return;
            t0 = performance.now();
            f0 = Math.max(a, Math.min(b - 1, af));
            if (get().frame !== f0) set({ frame: f0 });
          })
          .catch(() => {})
          .finally(() => (asking = false));
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      stopped = true;
      cancelAnimationFrame(raf);
      void api.stop();
    };
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
    void refreshPlugins(true);
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
    <TooltipProvider delay={500}>
      <div className={`app ${showProject ? "" : "no-left"} ${showInspector ? "" : "no-right"}`}>
        <header className="titlebar" data-tauri-drag-region>
          <span className="titlebar-left" data-tauri-drag-region>
            <IconButton icon="sidebarL" title="プロジェクト (⌘1)" on={showProject} onClick={() => set({ showProject: !showProject })} />
            <PluginToolbar />
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
        <PluginPanels />
        <PluginManager />
        <PluginLog />
        <PluginNotices />
      </div>
    </TooltipProvider>
  );
}
