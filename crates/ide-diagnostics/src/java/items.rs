//! Java item-tree walks used by the Java diagnostics collector.

use hir::hir_def::java::item_tree::{ItemData, ItemId, ItemTree};

pub(crate) fn find_method(tree: &ItemTree, id: ItemId, name: &str) -> Option<ItemId> {
    match tree.data(id) {
        ItemData::Method(method) if method.name.as_str() == name => return Some(id),
        _ => {}
    }
    for &child in tree.data(id).body() {
        if let Some(found) = find_method(tree, child, name) {
            return Some(found);
        }
    }
    None
}

/// Every `(ItemId, &ItemData)` in the tree, parents before children.
pub(crate) fn all_items(tree: &ItemTree) -> Vec<(ItemId, &ItemData)> {
    fn walk<'a>(tree: &'a ItemTree, id: ItemId, out: &mut Vec<(ItemId, &'a ItemData)>) {
        let data = tree.data(id);
        out.push((id, data));
        for &child in data.body() {
            walk(tree, child, out);
        }
        // A local class-like declaration ([JLS §14.3]) is not a member, so it
        // is not in any `body()`: it and its members are walked from the
        // declaration whose body declares it, so their bodies are inferred
        // and their diagnostics reported too.
        for local in tree.local_types_of(id) {
            walk(tree, local, out);
        }
    }
    let mut out = Vec::new();
    for &top in &tree.top {
        walk(tree, top, &mut out);
    }
    out
}
