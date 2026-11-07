// 画面全体。左にプロジェクト、中央にビューアとタイムライン、右にインスペクタ。
// メニューバーの項目とキー操作もここで受ける（Rust からは項目の ID が届くだけで、何をするかはここで決める）。

import { useEffect } from "react";
import { api } from "./api";
import { Inspector } from "./components/Inspector";
import { IconButton } from "./components/controls";
import { layerActions } from "./components/layerActions";
import { newComposition, ProjectList } from "./components/ProjectList";
import { Timeline } from "./components/Timeline";
import { createLayer, penKey, Viewer } from "./components/Viewer";
import { fps } from "./model";
import { adopt, currentComp, get, keysOp, reply, run, set, totalFrames, useUi } from "./store";
import type { KeyKind } from "./model";
import {
  CheckDialog,
  ExportDialog,
  HistoryDialog,
  LicensesDialog,
  RecoveryDialog,
  ReviewsDialog,
  runCheck,
  TaskIndicator,
  useTasks,
  VersionsDialog,
} from "./components/Windows";
import { addMarker, copyKeys, pasteKeys, runAction, runOp, selectAllKeys } from "./actions";
import { actionFor, loadKeymap } from "./keymap";
import { BeatfitDialog, ShortcutsDialog } from "./components/Tools";
import { GraphWindow, ScriptDialog } from "./components/Graph";
import { onPluginKey, onPluginMenu, PluginLog, PluginManager, PluginNotices, PluginPanels, PluginToolbar, refreshPlugins } from "./components/Plugins";
import { TooltipProvider } from "@/components/ui/tooltip";

function onMenu(id: string) {
  const s = get();
  if (id.startsWith("layer:")) return void createLayer(id.slice(6));
  if (onPluginMenu(id)) return;
  if (id.startsWith("detect_beats:")) return void runOp({ op: "detect_beats", layer: s.selected, sensitivity: Number(id.slice(13)) });
  if (id.startsWith("band:")) {
    if (!s.selected) return set({ status: "スケールを付けるレイヤーを選択してください" });
    return void runOp({ op: "band_scale", layer: s.selected, source: s.selected, band: id.slice(5) });
  }
  if (id.startsWith("keys:")) return void keysOp("キーフレーム補間", { op: "kind", kind: id.slice(5) as KeyKind });
  switch (id) {
    case "new":
      return void run(api.newProject, "新しいプロジェクトを作りました");
    case "open":
      return void reply(() => api.open());
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
    case "import_lyrics":
      return void reply(() => api.importLyrics(s.compId));
    case "import_camera":
      return void reply(() => api.importCamera(s.compId));
    case "relink":
      return void reply(api.relinkFootage);
    case "collect":
      return void reply(api.collectFiles);
    case "export_comp_package":
      return void reply(() => api.exportCompPackage(s.compId));
    case "import_comp_package":
      return void reply(api.importCompPackage);
    case "save_frame":
      return void reply(() => api.saveFrameImage(s.frame, s.downsample));
    case "check":
      set({ dialog: "check" });
      if (!s.check && !s.task) runCheck();
      return;
    case "copy_credits":
      return void api.creditsText(s.compId).then((t) => copy(t, "クレジット文面をクリップボードにコピーしました"));
    case "copy_log":
      return void api.productionLog().then((t) => copy(t, "制作ログをクリップボードにコピーしました"));
    case "breakdown":
      return void reply(() => api.exportBreakdown(s.compId, s.frame));
    case "export":
    case "history":
    case "versions":
    case "reviews":
    case "licenses":
      return set({ dialog: id });
    case "undo":
      if (isTyping()) return void document.execCommand("undo");
      return void run(api.undo);
    case "redo":
      if (isTyping()) return void document.execCommand("redo");
      return void run(api.redo);
    case "duplicate":
      return s.selected && layerActions(s.selected).duplicate();
    case "bring_forward":
      return s.selected && layerActions(s.selected).reorder("forward");
    case "send_backward":
      return s.selected && layerActions(s.selected).reorder("backward");
    case "bring_front":
      return s.selected && layerActions(s.selected).reorder("front");
    case "send_back":
      return s.selected && layerActions(s.selected).reorder("back");
    case "split":
    case "precompose":
    case "outline":
    case "new_row":
      return s.selected && void runOp({ op: id, layer: s.selected });
    case "marker":
      return addMarker();
    case "clear_markers":
      return void runOp({ op: "clear_markers" });
    case "bpm_from_taps":
      return void runOp({ op: "bpm_from_taps" });
    case "select_all_keys":
      return void selectAllKeys();
    case "reverse_keys":
      return runAction("reverse_keys");
    case "work_clear":
      return set({ workArea: null });
    case "graph":
      return set((st) => ({ graphOpen: !st.graphOpen }));
    case "hide_shy":
      return set((st) => ({ hideShy: !st.hideShy }));
    case "only_animated":
      return set((st) => ({ onlyAnimated: !st.onlyAnimated }));
    case "screening":
      return set((st) => ({ screening: !st.screening }));
    case "shortcuts":
    case "beatfit":
    case "script":
      return set({ dialog: id });
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

function copy(text: string, ok: string) {
  void navigator.clipboard.writeText(text).then(
    () => set({ status: ok }),
    () => set({ status: "クリップボードに入れられませんでした" }),
  );
}

const isTyping = () => {
  const el = document.activeElement;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT");
};

/** キー操作。割り当て（keymap.ts）から操作を探して実行する。⌘ のつくものの多くはメニューが先に受ける。 */
function onKey(e: KeyboardEvent) {
  const typing = isTyping();
  if (!typing && get().tool === "pen" && (e.key === "Enter" || e.key === "Escape")) {
    // 描きかけのペンがあれば確定・取り消し（無ければ通常の Esc）
    void penKey(e.key).then((done) => !done && e.key === "Escape" && set({ selected: null, selectedFootage: null, selectedKeys: null }));
    return;
  }
  if (!typing && onPluginKey(e)) return;
  if (!typing && e.key === "Escape") return set({ selected: null, selectedFootage: null, selectedKeys: null });
  const action = actionFor(e);
  if (!action) {
    // 割り当ての無いキー: T でテキストを作る
    if (!typing && !e.metaKey && !e.ctrlKey && !e.altKey && e.key === "t") void createLayer("text");
    return;
  }
  if (typing && !action.global) return;
  if (e.repeat && !action.repeat) return;
  e.preventDefault();
  runAction(action.id);
}

/** ⌘C / ⌘V: 文字入力中でなければ、キーフレームのコピーと貼り付け。 */
function onCopy(e: ClipboardEvent) {
  if (!isTyping() && copyKeys()) e.preventDefault();
}
function onPaste(e: ClipboardEvent) {
  if (isTyping()) return;
  const text = e.clipboardData?.getData("text/plain") ?? "";
  if (text) {
    e.preventDefault();
    void pasteKeys(text);
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

/** エクスプレッションのエラーを、編集があったときだけ調べ直す（少し待ってから）。 */
function useExprErrors() {
  const revision = useUi((s) => s.snap?.revision ?? 0);
  const compId = useUi((s) => s.compId);
  useEffect(() => {
    const id = setTimeout(() => {
      void api.exprErrors(compId, get().frame).then((exprErrors) => set({ exprErrors })).catch(() => {});
    }, 300);
    return () => clearTimeout(id);
  }, [revision, compId]);
}

export function App() {
  const ready = useUi((s) => !!s.project);
  const showProject = useUi((s) => s.showProject);
  const showInspector = useUi((s) => s.showInspector);
  const title = useUi((s) => (s.snap?.path ? s.snap.path.split(/[\\/]/).pop()! : "未保存"));
  const dirty = useUi((s) => !!s.snap?.dirty);
  const status = useUi((s) => s.status);
  usePlayback();
  useTasks();
  useExprErrors();

  useEffect(() => {
    void api.snapshot().then(adopt);
    void refreshPlugins(true);
    const un = api.onMenu(onMenu);
    void loadKeymap();
    window.addEventListener("keydown", onKey);
    document.addEventListener("copy", onCopy);
    document.addEventListener("paste", onPaste);
    return () => {
      void un.then((u) => u());
      window.removeEventListener("keydown", onKey);
      document.removeEventListener("copy", onCopy);
      document.removeEventListener("paste", onPaste);
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
            <TaskIndicator />
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
        <ExportDialog />
        <CheckDialog />
        <HistoryDialog />
        <VersionsDialog />
        <ReviewsDialog />
        <LicensesDialog />
        <RecoveryDialog />
        <BeatfitDialog />
        <ShortcutsDialog />
        <GraphWindow />
        <ScriptDialog />
      </div>
    </TooltipProvider>
  );
}
