//! macOS のメニューバー。項目を選ぶと、その ID を "menu" イベントで画面に送る（何をするかは画面が決める）。
//! 切り取り・コピー・貼り付けは OS 標準の項目にする（これが無いと入力欄で ⌘C / ⌘V が効かない）。

use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Runtime};

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
    MenuBuilder::new(app).items(&[&app_menu, &file, &edit, &layer, &view]).build()
}

pub fn forward<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let _ = app.emit("menu", id.to_string());
}
