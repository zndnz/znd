//! 少人数での共同制作: コンポ単位のロック、コンポの書き出し／読み込み、フレーム指定のレビューコメント。

use crate::model::{CompId, Composition, FootageId, LayerKind, Project, ReviewComment};
use crate::rights::used_footage;
use crate::time::Time;
use std::collections::{BTreeSet, HashMap};

/// コンポをロックする。既に他の人がロックしていれば、その人の名前を `Err` で返す。
pub fn lock_comp(project: &mut Project, comp: &CompId, user: &str) -> Result<(), String> {
    let c = project.comp_mut(comp).ok_or_else(|| format!("コンポ「{comp}」が見つかりません"))?;
    match &c.locked_by {
        Some(owner) if owner != user => Err(owner.clone()),
        _ => {
            c.locked_by = Some(user.to_string());
            Ok(())
        }
    }
}

/// ロックを外す。ロックした本人だけが外せる。
pub fn unlock_comp(project: &mut Project, comp: &CompId, user: &str) -> Result<(), String> {
    let c = project.comp_mut(comp).ok_or_else(|| format!("コンポ「{comp}」が見つかりません"))?;
    match &c.locked_by {
        Some(owner) if owner != user => Err(owner.clone()),
        _ => {
            c.locked_by = None;
            Ok(())
        }
    }
}

/// `user` がこのコンポを編集してよいか。
pub fn can_edit(comp: &Composition, user: &str) -> bool {
    comp.locked_by.as_deref().is_none_or(|owner| owner == user)
}

/// コンポと、それが使うネストしたコンポ・フッテージだけを取り出した小さなプロジェクトを作る。
/// 先頭のコンポが書き出したコンポ本体。
pub fn export_comp(project: &Project, comp: &CompId) -> Option<Project> {
    let root = project.comp(comp)?;
    let mut ids: Vec<CompId> = Vec::new();
    fn walk(project: &Project, comp: &Composition, ids: &mut Vec<CompId>) {
        if ids.contains(&comp.id) {
            return;
        }
        ids.push(comp.id.clone());
        for l in &comp.layers {
            if let LayerKind::Precomp { comp: nested } = &l.kind {
                if let Some(n) = project.comp(nested) {
                    walk(project, n, ids);
                }
            }
        }
    }
    walk(project, root, &mut ids);
    let mut out = Project::new(root.name.clone());
    out.settings = project.settings.clone();
    out.compositions = ids.iter().filter_map(|id| project.comp(id).cloned()).collect();
    // ガイドレイヤーの素材も、編集を続けるために必要なので含める
    let mut footage: BTreeSet<FootageId> = used_footage(project, comp).into_iter().map(|f| f.id.clone()).collect();
    for c in &out.compositions {
        for l in &c.layers {
            if let LayerKind::Footage { footage: f } | LayerKind::Audio { footage: f } | LayerKind::Model { footage: f } = &l.kind {
                footage.insert(f.clone());
            }
        }
    }
    out.footage = project.footage.iter().filter(|f| footage.contains(&f.id)).cloned().collect();
    out.reviews = project.reviews.iter().filter(|r| ids.contains(&r.comp)).cloned().collect();
    Some(out)
}

/// `export_comp` で作ったパッケージを取り込む。ID が衝突したら付け替える。
/// 同じファイルを指すフッテージは既存のものを使い回す。取り込んだコンポ本体の ID を返す。
pub fn import_comp(project: &mut Project, package: &Project) -> Option<CompId> {
    let root = package.compositions.first()?.id.clone();
    // フッテージ: パスが同じなら既存を再利用
    let mut footage_map: HashMap<FootageId, FootageId> = HashMap::new();
    for f in &package.footage {
        if let Some(existing) = project.footage.iter().find(|e| e.path == f.path) {
            footage_map.insert(f.id.clone(), existing.id.clone());
            continue;
        }
        let mut item = f.clone();
        if project.footage_item(&f.id).is_some() {
            item.id = FootageId::new(project.fresh_id("footage"));
        }
        footage_map.insert(f.id.clone(), item.id.clone());
        project.footage.push(item);
    }
    // コンポ: ID が衝突したら新しい ID にする（レイヤー ID はコンポ内で一意なのでそのまま）
    let mut comp_map: HashMap<CompId, CompId> = HashMap::new();
    let mut taken: Vec<String> = project.compositions.iter().map(|c| c.id.0.clone()).collect();
    for c in &package.compositions {
        let mut id = c.id.0.clone();
        let mut n = 2;
        while taken.contains(&id) {
            id = format!("{}-{n}", c.id.0);
            n += 1;
        }
        taken.push(id.clone());
        comp_map.insert(c.id.clone(), CompId::new(id));
    }
    for c in &package.compositions {
        let mut comp = c.clone();
        comp.id = comp_map[&c.id].clone();
        comp.locked_by = None;
        for l in &mut comp.layers {
            match &mut l.kind {
                LayerKind::Footage { footage } | LayerKind::Audio { footage } | LayerKind::Model { footage } => {
                    if let Some(mapped) = footage_map.get(footage) {
                        *footage = mapped.clone();
                    }
                }
                LayerKind::Precomp { comp } => {
                    if let Some(mapped) = comp_map.get(comp) {
                        *comp = mapped.clone();
                    }
                }
                _ => {}
            }
        }
        project.compositions.push(comp);
    }
    for r in &package.reviews {
        let mut r = r.clone();
        if let Some(mapped) = comp_map.get(&r.comp) {
            r.comp = mapped.clone();
        }
        project.reviews.push(r);
    }
    comp_map.get(&root).cloned()
}

/// レビューコメントを追加する。
pub fn add_review(project: &mut Project, comp: &CompId, time: Time, author: &str, text: &str, point: Option<[f64; 2]>, created: &str) {
    project.reviews.push(ReviewComment {
        comp: comp.clone(),
        time,
        author: author.into(),
        text: text.into(),
        point,
        resolved: false,
        created: created.into(),
    });
    project.reviews.sort_by(|a, b| (a.comp.0.as_str(), a.time).cmp(&(b.comp.0.as_str(), b.time)));
}

/// コンポの未解決コメント（時刻順）。
pub fn open_reviews<'a>(project: &'a Project, comp: &CompId) -> Vec<&'a ReviewComment> {
    project.reviews.iter().filter(|r| &r.comp == comp && !r.resolved).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_project;

    fn main() -> CompId {
        CompId::new("main")
    }

    #[test]
    fn locks_are_owned() {
        let mut p = demo_project();
        assert!(can_edit(&p.compositions[0], "あい"));
        lock_comp(&mut p, &main(), "あい").unwrap();
        assert!(can_edit(&p.compositions[0], "あい") && !can_edit(&p.compositions[0], "うえ"));
        assert_eq!(lock_comp(&mut p, &main(), "うえ"), Err("あい".into()));
        assert_eq!(unlock_comp(&mut p, &main(), "うえ"), Err("あい".into()));
        // ロックはプロジェクトファイルに保存される（共有フォルダ越しに相手にも見える）
        let json = crate::io::to_json(&p);
        assert!(json.contains("\"locked_by\": \"あい\""));
        unlock_comp(&mut p, &main(), "あい").unwrap();
        assert!(can_edit(&p.compositions[0], "うえ"));
        assert!(lock_comp(&mut p, &CompId::new("無い"), "あい").is_err());
    }

    #[test]
    fn export_then_import_keeps_structure_and_remaps_ids() {
        let p = demo_project();
        let pkg = export_comp(&p, &main()).unwrap();
        assert_eq!(pkg.compositions.iter().map(|c| c.id.0.as_str()).collect::<Vec<_>>(), vec!["main", "bars"]);
        assert_eq!(pkg.validate(), Vec::<String>::new());
        // ネストしたコンポだけを書き出すと、それだけが入る
        assert_eq!(export_comp(&p, &CompId::new("bars")).unwrap().compositions.len(), 1);

        // 同じプロジェクトへ取り込むと ID が衝突するので付け替えられ、内部の参照も追従する
        let mut dest = demo_project();
        let id = import_comp(&mut dest, &pkg).unwrap();
        assert_eq!(id, CompId::new("main-2"));
        assert_eq!(dest.compositions.len(), 4);
        assert_eq!(dest.validate(), Vec::<String>::new());
        let imported = dest.comp(&id).unwrap();
        let pre = imported.layers.iter().find_map(|l| match &l.kind {
            LayerKind::Precomp { comp } => Some(comp.clone()),
            _ => None,
        });
        assert_eq!(pre, Some(CompId::new("bars-2")));
        // 元のコンポは変わらない
        assert_eq!(dest.comp(&main()), p.comp(&main()));
        assert!(export_comp(&p, &CompId::new("無い")).is_none());
    }

    #[test]
    fn reviews_are_sorted_and_filtered() {
        let mut p = demo_project();
        add_review(&mut p, &main(), Time::from_seconds(3.0), "うえ", "ここ音とずれてる", Some([960.0, 540.0]), "2026-10-02T10:00:00");
        add_review(&mut p, &main(), Time::from_seconds(1.0), "あい", "色が暗い", None, "2026-10-02T10:05:00");
        add_review(&mut p, &CompId::new("bars"), Time::ZERO, "あい", "別コンポ", None, "");
        let open = open_reviews(&p, &main());
        assert_eq!(open.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), vec!["色が暗い", "ここ音とずれてる"]);
        p.reviews.iter_mut().find(|r| r.text == "色が暗い").unwrap().resolved = true;
        assert_eq!(open_reviews(&p, &main()).len(), 1);
        // コメントはコンポと一緒に書き出される
        assert_eq!(export_comp(&p, &main()).unwrap().reviews.len(), 3);
        let back: Project = serde_json::from_str(&crate::io::to_json(&p)).unwrap();
        assert_eq!(back, p);
    }
}
