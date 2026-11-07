//! macOS のメニューバー。項目を選ぶと、その ID を "menu" イベントで画面に送る（何をするかは画面が決める）。
//! 切り取り・コピー・貼り付けは OS 標準の項目にする（これが無いと入力欄で ⌘C / ⌘V が効かない）。

use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Runtime};
use znd_render::plugins::{self, hooks};

/// プラグインの "Cmd+Shift+K" のような表記を、メニューのショートカットの表記にする。
fn accelerator(s: &str) -> String {
    s.split('+')
        .map(|p| match p.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "mod" => "CmdOrCtrl".to_string(),
            "ctrl" | "control" => "Ctrl".to_string(),
            "alt" | "option" | "opt" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            _ => p.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// 「プラグイン」メニュー。コマンド（パッケージごと）・パネル・出力フィルター・管理。
fn plugin_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Submenu<R>> {
    let mut b = SubmenuBuilder::new(app, "プラグイン");
    let commands: Vec<_> = plugins::commands().into_iter().filter(|c| c.menu.is_empty() || c.menu.iter().any(|m| m == "plugins")).collect();
    let mut last_plugin: Option<String> = None;
    for c in &commands {
        if last_plugin.as_deref() != Some(c.plugin_name.as_str()) {
            if last_plugin.is_some() {
                b = b.separator();
            }
            b = b.item(&MenuItemBuilder::new(&c.plugin_name).enabled(false).build(app)?);
            last_plugin = Some(c.plugin_name.clone());
        }
        let id = format!("plugin_cmd:{}", c.key);
        // ショートカットの表記がメニューで使えないものなら、ショートカット無しで出す
        let item = match c.shortcut.as_deref() {
            Some(sc) => MenuItemBuilder::with_id(&id, &c.label).accelerator(accelerator(sc)).build(app).or_else(|_| MenuItemBuilder::with_id(&id, &c.label).build(app))?,
            None => MenuItemBuilder::with_id(&id, &c.label).build(app)?,
        };
        b = b.item(&item);
    }
    if commands.is_empty() {
        b = b.item(&MenuItemBuilder::new("コマンドを持つプラグインはありません").enabled(false).build(app)?);
    }
    let panels = plugins::panels();
    if !panels.is_empty() {
        b = b.separator().item(&MenuItemBuilder::new("パネル").enabled(false).build(app)?);
        for p in panels {
            b = b.item(&MenuItemBuilder::with_id(format!("plugin_panel:{}", p.key), &p.title).build(app)?);
        }
    }
    let filters = hooks::output_filters();
    if !filters.is_empty() {
        b = b.separator().item(&MenuItemBuilder::new("出力フィルター（コンポの最終画像に掛ける）").enabled(false).build(app)?);
        for f in filters {
            let item = CheckMenuItemBuilder::with_id(format!("plugin_filter:{}", f.key), format!("{}（{}）", f.label, f.stage.label())).checked(f.enabled).build(app)?;
            b = b.item(&item);
        }
    }
    b.separator()
        .item(&MenuItemBuilder::with_id("plugins_manage", "プラグインの管理と安全性…").build(app)?)
        .item(&MenuItemBuilder::with_id("plugins_log", "プラグインのログ…").build(app)?)
        .build()
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let item = |id: &str, text: &str, key: Option<&str>| {
        let b = MenuItemBuilder::with_id(id, text);
        match key {
            Some(k) => b.accelerator(k).build(app),
            None => b.build(app),
        }
    };
    let app_menu = SubmenuBuilder::new(app, "znd").about(None).separator().hide().hide_others().separator().quit().build()?;
    let file = SubmenuBuilder::new(app, "ファイル")
        .item(&item("new", "新規プロジェクト", Some("CmdOrCtrl+N"))?)
        .item(&item("open", "開く…", Some("CmdOrCtrl+O"))?)
        .separator()
        .item(&item("save", "保存", Some("CmdOrCtrl+S"))?)
        .item(&item("save_as", "別名で保存…", Some("Shift+CmdOrCtrl+S"))?)
        .separator()
        .item(&item("import", "素材を読み込む…", Some("CmdOrCtrl+I"))?)
        .item(&item("import_sequence", "連番画像を読み込む…", None)?)
        .item(&item("import_lyrics", "歌詞を読み込む（LRC / SRT / ASS）…", None)?)
        .item(&item("import_camera", "カメラを読み込む（.chan / Alembic）…", None)?)
        .separator()
        .item(&item("relink", "欠落した素材を再リンク…", None)?)
        .item(&item("collect", "ファイルを収集（権利情報付き）…", None)?)
        .item(&item("export_comp_package", "このコンポを書き出す（受け渡し用）…", None)?)
        .item(&item("import_comp_package", "コンポを読み込む…", None)?)
        .separator()
        .item(&item("export", "書き出し…", Some("CmdOrCtrl+M"))?)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "編集")
        .item(&item("undo", "取り消す", Some("CmdOrCtrl+Z"))?)
        .item(&item("redo", "やり直す", Some("Shift+CmdOrCtrl+Z"))?)
        .item(&item("history", "履歴…", None)?)
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .separator()
        .item(&item("duplicate", "複製", Some("CmdOrCtrl+D"))?)
        .build()?;
    let comp = SubmenuBuilder::new(app, "コンポジション")
        .item(&item("new_comp", "新規コンポジション", Some("Alt+CmdOrCtrl+N"))?)
        .separator()
        .item(&item("save_frame", "フレームを画像として保存…", Some("Alt+CmdOrCtrl+S"))?)
        .separator()
        .item(&item("check", "提出前チェック…", None)?)
        .item(&item("copy_credits", "クレジット文面をコピー", None)?)
        .item(&item("copy_log", "制作ログをコピー", None)?)
        .item(&item("breakdown", "メイキング用の分解を書き出す…", None)?)
        .separator()
        .item(&item("versions", "版（スナップショット）…", None)?)
        .item(&item("reviews", "レビューコメント…", None)?)
        .build()?;
    let layer = SubmenuBuilder::new(app, "レイヤー")
        .item(&item("layer:text", "テキスト", None)?)
        .item(&item("layer:rect", "シェイプ（矩形）", None)?)
        .item(&item("layer:ellipse", "シェイプ（楕円）", None)?)
        .item(&item("layer:star", "シェイプ（スター）", None)?)
        .item(&item("layer:solid", "平面", None)?)
        .item(&item("layer:null", "ヌル", None)?)
        .item(&item("layer:adjustment", "調整レイヤー", None)?)
        .item(&item("layer:camera", "カメラ", None)?)
        .item(&item("layer:light_point", "ライト", None)?)
        .separator()
        .item(&item("split", "再生ヘッドで分割", None)?)
        .item(&item("precompose", "プリコンポーズ", None)?)
        .item(&item("outline", "テキストをシェイプに変換", None)?)
        .item(&item("new_row", "この行から出す", None)?)
        .separator()
        .item(&item("bring_front", "最前面へ", Some("Shift+CmdOrCtrl+]"))?)
        .item(&item("bring_forward", "前面へ", Some("CmdOrCtrl+]"))?)
        .item(&item("send_backward", "背面へ", Some("CmdOrCtrl+["))?)
        .item(&item("send_back", "最背面へ", Some("Shift+CmdOrCtrl+["))?)
        .build()?;
    let audio = SubmenuBuilder::new(app, "オーディオから生成")
        .item(&item("detect_beats:0.3", "ビートを検出してマーカーを打つ（控えめ）", None)?)
        .item(&item("detect_beats:0.5", "ビートを検出してマーカーを打つ（標準）", None)?)
        .item(&item("detect_beats:0.8", "ビートを検出してマーカーを打つ（細かく）", None)?)
        .separator()
        .item(&item("band:low", "低域の音量 → 選択レイヤーのスケール", None)?)
        .item(&item("band:mid", "中域の音量 → 選択レイヤーのスケール", None)?)
        .item(&item("band:high", "高域の音量 → 選択レイヤーのスケール", None)?)
        .separator()
        .item(&item("bpm_from_taps", "空コメントのマーカーから BPM を推定（タップ入力）", None)?)
        .item(&item("clear_markers", "マーカーをすべて削除", None)?)
        .build()?;
    let anim = SubmenuBuilder::new(app, "アニメーション")
        .item(&item("select_all_keys", "選択レイヤーのキーフレームをすべて選択", None)?)
        .item(&item("keys:easy_ease", "イージーイーズ", None)?)
        .item(&item("keys:ease_in", "イーズイン", None)?)
        .item(&item("keys:ease_out", "イーズアウト", None)?)
        .item(&item("keys:linear", "リニア", None)?)
        .item(&item("keys:hold", "停止", None)?)
        .item(&item("reverse_keys", "キーフレームの時間反転", None)?)
        .item(&item("graph", "グラフエディター", None)?)
        .item(&item("only_animated", "動きのあるプロパティだけを表示", None)?)
        .separator()
        .item(&item("marker", "現在の時刻にマーカーを追加", None)?)
        .item(&item("work_clear", "ワークエリアを解除", None)?)
        .separator()
        .item(&audio)
        .item(&item("beatfit", "ビートに合わせる…", None)?)
        .build()?;
    let view = SubmenuBuilder::new(app, "表示")
        .item(&item("toggle_project", "プロジェクトを表示", Some("CmdOrCtrl+1"))?)
        .item(&item("toggle_inspector", "インスペクタを表示", Some("CmdOrCtrl+2"))?)
        .separator()
        .item(&item("zoom_fit", "ビューアを全体表示", Some("Shift+/"))?)
        .item(&item("timeline_fit", "タイムラインを全体表示", None)?)
        .item(&item("screening", "上映プレビュー", None)?)
        .item(&item("hide_shy", "シャイのレイヤーを隠す", None)?)
        .separator()
        .item(&item("script", "スクリプト…", None)?)
        .build()?;
    let plugin = plugin_menu(app)?;
    let help = SubmenuBuilder::new(app, "ヘルプ")
        .item(&item("shortcuts", "キーボードショートカット…", None)?)
        .item(&item("licenses", "使用 OSS とライセンス…", None)?)
        .build()?;
    MenuBuilder::new(app).items(&[&app_menu, &file, &edit, &comp, &layer, &anim, &view, &plugin, &help]).build()
}

pub fn forward<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let _ = app.emit("menu", id.to_string());
}
