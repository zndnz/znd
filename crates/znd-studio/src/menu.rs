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
        .item(&item("new_comp", "新規コンポジション", Some("Alt+CmdOrCtrl+N"))?)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "編集")
        .item(&item("undo", "取り消す", Some("CmdOrCtrl+Z"))?)
        .item(&item("redo", "やり直す", Some("Shift+CmdOrCtrl+Z"))?)
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .separator()
        .item(&item("duplicate", "複製", Some("CmdOrCtrl+D"))?)
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
        .item(&item("bring_forward", "前面へ", Some("CmdOrCtrl+]"))?)
        .item(&item("send_backward", "背面へ", Some("CmdOrCtrl+["))?)
        .build()?;
    let view = SubmenuBuilder::new(app, "表示")
        .item(&item("toggle_project", "プロジェクトを表示", Some("CmdOrCtrl+1"))?)
        .item(&item("toggle_inspector", "インスペクタを表示", Some("CmdOrCtrl+2"))?)
        .separator()
        .item(&item("zoom_fit", "ビューアを全体表示", Some("Shift+/"))?)
        .item(&item("timeline_fit", "タイムラインを全体表示", None)?)
        .build()?;
    let plugin = plugin_menu(app)?;
    MenuBuilder::new(app).items(&[&app_menu, &file, &edit, &layer, &view, &plugin]).build()
}

pub fn forward<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let _ = app.emit("menu", id.to_string());
}
