//! 権利管理: 使用素材の列挙、許諾未確認の警告、クレジット文面の生成。

use crate::model::{Clearance, CompId, FootageId, FootageItem, LayerKind, Project};
use crate::text::TextDocument;
use std::collections::BTreeSet;

/// コンポで実際に使われているフッテージ（ネストしたコンポ内も含む）。プロジェクトへの登録順。
pub fn used_footage<'a>(project: &'a Project, comp: &CompId) -> Vec<&'a FootageItem> {
    let mut ids: BTreeSet<FootageId> = BTreeSet::new();
    let mut seen: BTreeSet<CompId> = BTreeSet::new();
    collect(project, comp, &mut ids, &mut seen);
    project.footage.iter().filter(|f| ids.contains(&f.id)).collect()
}

fn collect(project: &Project, comp: &CompId, ids: &mut BTreeSet<FootageId>, seen: &mut BTreeSet<CompId>) {
    if !seen.insert(comp.clone()) {
        return;
    }
    let Some(comp) = project.comp(comp) else { return };
    for layer in &comp.layers {
        // 非表示でも、トラックマットやエフェクトの参照先として使われうるので数える（ガイドレイヤーは書き出されないので除く）
        if layer.guide {
            continue;
        }
        match &layer.kind {
            LayerKind::Footage { footage } | LayerKind::Audio { footage } | LayerKind::Model { footage } => {
                ids.insert(footage.clone());
            }
            LayerKind::Precomp { comp } => collect(project, comp, ids, seen),
            _ => {}
        }
    }
}

/// コンポで使われているフォントファミリー（重複なし）。空文字（既定フォント）は含めない。
pub fn used_fonts(project: &Project, comp: &CompId) -> Vec<String> {
    fn walk(project: &Project, comp: &CompId, out: &mut BTreeSet<String>, seen: &mut BTreeSet<CompId>) {
        if !seen.insert(comp.clone()) {
            return;
        }
        let Some(comp) = project.comp(comp) else { return };
        for layer in &comp.layers {
            match &layer.kind {
                LayerKind::Text { document: TextDocument { font_family, .. }, .. } if !font_family.is_empty() => {
                    out.insert(font_family.clone());
                }
                LayerKind::Precomp { comp } => walk(project, comp, out, seen),
                _ => {}
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(project, comp, &mut out, &mut BTreeSet::new());
    out.into_iter().collect()
}

fn label(c: Clearance) -> &'static str {
    match c {
        Clearance::Unconfirmed => "未確認",
        Clearance::Cleared => "許諾済み",
        Clearance::NotApplicable => "該当なし",
        Clearance::Denied => "不許可",
    }
}

/// 1 つの素材について、問題のある権利項目を「著作権: 未確認」の形で列挙する。
pub fn issues(item: &FootageItem) -> Vec<String> {
    let r = &item.rights;
    [("著作権", r.copyright), ("著作隣接権・原盤権", r.neighboring), ("肖像権", r.portrait), ("商標", r.trademark)]
        .into_iter()
        .filter(|(_, c)| matches!(c, Clearance::Unconfirmed | Clearance::Denied))
        .map(|(name, c)| format!("{name}: {}", label(c)))
        .collect()
}

/// 書き出し前の警告。許諾未確認・不許可の素材が残っていれば、素材ごとに 1 行返す。
pub fn export_warnings(project: &Project, comp: &CompId) -> Vec<String> {
    used_footage(project, comp)
        .into_iter()
        .filter_map(|item| {
            let i = issues(item);
            (!i.is_empty()).then(|| format!("「{}」の権利が確認できていません（{}）", item.name, i.join("、")))
        })
        .collect()
}

/// エンディングや概要欄に貼るクレジット文面。作者ごとにまとめ、入手元とライセンスを添える。
pub fn credits(project: &Project, comp: &CompId) -> String {
    let mut out = String::from("■ 使用素材\n");
    let items = used_footage(project, comp);
    if items.is_empty() {
        out.push_str("（外部素材なし）\n");
    }
    // 作者未記入のものは最後にまとめる
    let mut authors: Vec<&str> = items.iter().map(|f| f.rights.author.as_str()).filter(|a| !a.is_empty()).collect();
    authors.sort();
    authors.dedup();
    for author in authors.iter().copied().chain(items.iter().any(|f| f.rights.author.is_empty()).then_some("")) {
        out.push_str(&format!("{}\n", if author.is_empty() { "（作者未記入）" } else { author }));
        for f in items.iter().filter(|f| f.rights.author == author) {
            let mut line = format!("  ・{}", f.name);
            if !f.rights.license.is_empty() {
                line.push_str(&format!("（{}）", f.rights.license));
            }
            if !f.rights.source_url.is_empty() {
                line.push_str(&format!(" {}", f.rights.source_url));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    let fonts = used_fonts(project, comp);
    if !fonts.is_empty() {
        out.push_str("\n■ 使用フォント\n");
        for f in fonts {
            out.push_str(&format!("  ・{f}\n"));
        }
    }
    out.push_str("\n■ 制作ツール\n  ・znd\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::time::{FrameRate, Time};

    fn item(id: &str, author: &str, copyright: Clearance) -> FootageItem {
        FootageItem {
            id: FootageId::new(id),
            name: format!("{id}.png"),
            path: format!("{id}.png"),
            width: 10,
            height: 10,
            frame_rate: None,
            duration: None,
            rights: Rights {
                author: author.into(),
                copyright,
                neighboring: Clearance::NotApplicable,
                portrait: Clearance::NotApplicable,
                trademark: Clearance::NotApplicable,
                license: "CC BY 4.0".into(),
                source_url: "https://example.com/x".into(),
                ..Default::default()
            },
            audio: None,
            vfr: false,
        }
    }

    fn footage_layer(id: &str) -> Layer {
        Layer::new(
            id,
            id,
            LayerKind::Footage { footage: FootageId::new(id) },
            Transform::centered(10.0, 10.0, 10.0, 10.0),
            Time::from_seconds(1.0),
        )
    }

    fn project() -> Project {
        let d = Time::from_seconds(1.0);
        let mut p = Project::new("r");
        p.footage = vec![
            item("a", "作者A", Clearance::Cleared),
            item("b", "作者B", Clearance::Unconfirmed),
            item("unused", "作者C", Clearance::Unconfirmed),
            item("g", "作者D", Clearance::Denied),
        ];
        let mut guide = footage_layer("g");
        guide.guide = true;
        let pre =
            Layer::new("pre", "pre", LayerKind::Precomp { comp: CompId::new("nested") }, Transform::centered(10.0, 10.0, 10.0, 10.0), d);
        let mut text = Layer::new(
            "t",
            "t",
            LayerKind::Text { document: TextDocument::new("x", 10.0), animators: vec![], path: None, per_char_3d: false },
            Transform::centered(0.0, 0.0, 10.0, 10.0),
            d,
        );
        if let LayerKind::Text { document, .. } = &mut text.kind {
            document.font_family = "源ノ角ゴシック".into();
        }
        p.compositions = vec![
            Composition {
                layers: vec![footage_layer("a"), pre, guide, text],
                ..Composition::new("main", "main", 10, 10, FrameRate::FPS_30, d)
            },
            Composition { layers: vec![footage_layer("b")], ..Composition::new("nested", "nested", 10, 10, FrameRate::FPS_30, d) },
        ];
        p
    }

    #[test]
    fn used_footage_follows_precomps_and_skips_guides_and_unused() {
        let p = project();
        let used: Vec<&str> = used_footage(&p, &CompId::new("main")).iter().map(|f| f.id.0.as_str()).collect();
        assert_eq!(used, vec!["a", "b"]);
        assert_eq!(used_fonts(&p, &CompId::new("main")), vec!["源ノ角ゴシック".to_string()]);
    }

    #[test]
    fn warnings_list_only_unconfirmed_used_material() {
        let p = project();
        let w = export_warnings(&p, &CompId::new("main"));
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("b.png") && w[0].contains("著作権: 未確認"));
        assert!(export_warnings(&p, &CompId::new("nested")).len() == 1);
    }

    #[test]
    fn credits_group_by_author() {
        let c = credits(&project(), &CompId::new("main"));
        assert!(c.contains("作者A\n  ・a.png（CC BY 4.0） https://example.com/x"));
        assert!(c.contains("作者B"));
        assert!(!c.contains("作者C") && !c.contains("作者D"));
        assert!(c.contains("■ 使用フォント\n  ・源ノ角ゴシック"));
    }
}
