//! Kotlin document symbols and the Kotlin outline helpers.

use rowan::TextRange;
use rustc_hash::FxHashSet;
use vfs::FileId;

use hir::hir_def::kotlin::item_tree::{KotlinItemData, KotlinItemTree};

use crate::RootDatabase;
use crate::symbols::{DocumentSymbol, ItemId};

/// The synthesized package symbol of a Kotlin file.
pub(super) fn package_symbol(db: &RootDatabase, file_id: FileId) -> DocumentSymbol {
    let Some(tree) = hir::hir_def::kotlin::plugin::tree(db, file_id) else {
        return crate::symbols::default_package_symbol(db, file_id);
    };
    let name = tree.package.as_ref().map_or_else(
        || "<default package>".to_owned(),
        |name| name.as_str().to_owned(),
    );
    let range = crate::symbols::range_ctx(db, file_id, tree.language)
        .and_then(|(map, source)| {
            hir::hir_def::kotlin::ranges::package_name_range(&map, &source, &tree)
        })
        .unwrap_or_default();
    DocumentSymbol {
        name: name.clone(),
        display_name: name,
        kind: hir::SourceSymbolKind::Package,
        range,
        name_range: range,
        detail: None,
        item: None,
    }
}

/// The declared range of `item` in a Kotlin file.
pub(super) fn source_symbol_range(
    db: &RootDatabase,
    file_id: FileId,
    item: ItemId,
) -> Option<TextRange> {
    if !hir::file_symbols(db, file_id)
        .iter()
        .any(|symbol| symbol.item == item)
    {
        return None;
    }
    let tree = hir::hir_def::kotlin::plugin::tree(db, file_id)?;
    let (map, source) = crate::symbols::range_ctx(db, file_id, tree.language)?;
    hir::hir_def::kotlin::ranges::item_range(&map, &source, &tree, item)
}

/// The declared symbols of a Kotlin file, in declaration order, prefixed by a
/// synthesized symbol for the file's package.
///
/// The Kotlin surface of [`document_symbols`]: a declaration's range is its
/// whole syntax node and its selection range its declared name, both resolved
/// from the item tree's anchors
/// ([`hir::hir_def::kotlin::ranges`]). The client-facing name carries the
/// signature the *item tree* knows — a function's parameters and return type,
/// a property's type — because the Kotlin type layer's rendering is not
/// available for an arbitrary declaration yet.
pub(super) fn document_symbols(db: &RootDatabase, file_id: FileId) -> Vec<DocumentSymbol> {
    let Some(tree) = hir::hir_def::kotlin::plugin::tree(db, file_id) else {
        return Vec::new();
    };
    let tree = &tree;
    let symbols = hir::file_symbols(db, file_id);
    let names: FxHashSet<&str> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
    let ctx = crate::symbols::range_ctx(db, file_id, tree.language);
    let mut out = Vec::with_capacity(symbols.len() + 1);
    if !symbols.is_empty() {
        out.push(package_symbol(db, file_id));
    }
    out.extend(symbols.iter().map(|source| {
        let top_level = source
            .name
            .as_str()
            .rsplit_once('.')
            .is_none_or(|(parent, _)| !names.contains(parent));
        let (range, name_range) = match ctx.as_ref() {
            Some((map, source_file)) => (
                hir::hir_def::kotlin::ranges::item_range(map, source_file, tree, source.item),
                hir::hir_def::kotlin::ranges::item_name_range(map, source_file, tree, source.item),
            ),
            None => (None, None),
        };
        let (range, name_range) = (range.unwrap_or_default(), name_range.unwrap_or_default());
        DocumentSymbol {
            name: source.name.as_str().to_owned(),
            kind: source.kind,
            range,
            name_range,
            display_name: kotlin_display_name(tree, source.item, source.name.simple_name()),
            detail: kotlin_detail(tree, source.item, source.name.simple_name(), top_level),
            item: Some(source.item),
        }
    }));
    out
}

/// The client-facing name of a Kotlin declaration: its simple name, with the
/// signature the item tree knows rendered inline — `name(params): ret` for a
/// function, `name: type` for a property.
fn kotlin_display_name(tree: &KotlinItemTree, item: ItemId, simple: &str) -> String {
    match tree.data(item) {
        KotlinItemData::Function(data) => {
            let params = data
                .params
                .iter()
                .map(|parameter| {
                    let param = &parameter.param;
                    format!(
                        "{}: {}",
                        param.name,
                        hir::hir_def::kotlin::pretty::display_type(&param.ty)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let ret = match &data.ret {
                Some(ret) => format!(": {}", hir::hir_def::kotlin::pretty::display_type(ret)),
                None => String::new(),
            };
            format!("{simple}({params}){ret}")
        }
        KotlinItemData::Property(data) => match &data.ty {
            Some(ty) => format!(
                "{simple}: {}",
                hir::hir_def::kotlin::pretty::display_type(ty)
            ),
            None => simple.to_owned(),
        },
        _ => simple.to_owned(),
    }
}

/// The `detail` of a Kotlin symbol: the fully qualified name for a top-level
/// declaration, `kind simple` for a nested one.
fn kotlin_detail(
    tree: &KotlinItemTree,
    item: ItemId,
    simple: &str,
    top_level: bool,
) -> Option<String> {
    if top_level {
        return Some(
            tree.data(item)
                .name()
                .map_or_else(|| simple.to_owned(), |name| name.as_str().to_owned()),
        );
    }
    Some(format!("{} {simple}", tree.data(item).label()))
}
