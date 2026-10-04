//! Java document symbols and the Java outline helpers.

use rowan::TextRange;
use rustc_hash::FxHashSet;
use triomphe::Arc;
use vfs::FileId;

use hir::hir_def::java::item_tree::ItemData;

use crate::RootDatabase;
use crate::symbols::{DocumentSymbol, ItemId};

/// The outline of a Java file: the symbols of its item tree, records expanded
/// with their implicit members.
///
/// A record ([JLS §8.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.10))
/// is expanded with its *implicit* members — a public accessor method per
/// component ([§8.10.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.10.3))
/// and its canonical constructor ([§8.10.4](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.10.4))
/// — so the outline shows what the language synthesizes. These are synthetic
/// (`item: None`) and carry no source ranges of their own: the accessor
/// ranges target its record component, and the canonical constructor's
/// ranges target the record declaration.
pub(super) fn document_symbols(db: &RootDatabase, file_id: FileId) -> Vec<DocumentSymbol> {
    let symbols = hir::file_symbols(db, file_id);
    let names: FxHashSet<&str> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
    let tree = hir::hir_def::java::plugin::tree(db, file_id);
    let ctx = crate::symbols::range_ctx(db, file_id, tree.language);
    let mut out = Vec::with_capacity(symbols.len() + 1);
    if !symbols.is_empty() {
        // The package is an independent item above the file's top-level
        // types; it is not a container of them.
        out.push(package_symbol(db, file_id));
    }
    out.extend(symbols.iter().flat_map(|source| {
        let top_level = source
            .name
            .as_str()
            .rsplit_once('.')
            .is_none_or(|(parent, _)| !names.contains(parent));
        let item = source.item;
        let Some(data) = hir_def_item_data(&tree, item) else {
            return Vec::new();
        };
        // The item tree carries no offsets: the ranges are resolved from the
        // file's parse. A record's outline range points at its declaration
        // *header* (the definition, including the component list) rather than
        // the whole body; its selection points at the component list itself.
        let (range, name_range) = match (ctx.as_ref(), data) {
            (Some((map, source)), ItemData::Record(_)) => (
                hir::hir_def::java::ranges::record_header_range(map, source, &tree, item),
                hir::hir_def::java::ranges::record_components_range(map, source, &tree, item),
            ),
            (Some((map, source)), _) => (
                hir::hir_def::java::ranges::item_range(map, source, &tree, item),
                hir::hir_def::java::ranges::item_name_range(map, source, &tree, item),
            ),
            // An unknown-language file has no parse; no ranges to point at.
            (None, _) => (None, None),
        };
        let (range, name_range) = (range.unwrap_or_default(), name_range.unwrap_or_default());
        let symbol = DocumentSymbol {
            name: source.name.as_str().to_owned(),
            kind: source.kind,
            range,
            name_range,
            display_name: symbol_display_name(db, file_id, source, top_level),
            detail: symbol_detail(source, top_level),
            item: Some(source.item),
        };
        std::iter::once(symbol.clone())
            .chain(record_members(db, file_id, &symbol, data))
            .collect::<Vec<_>>()
    }));
    out
}

/// The lowered item data of `item`, when the file's model declares it: a
/// rewritten file's tree can shrink under a stale id, so the item is bounds
/// checked before the arena is read.
fn hir_def_item_data<'a>(
    tree: &'a hir::hir_def::java::item_tree::ItemTree,
    item: ItemId,
) -> Option<&'a hir::hir_def::java::item_tree::ItemData> {
    ((item.0.0 as usize) < tree.items.len()).then(|| tree.data(item))
}

/// The implicit members of a record declaration, synthesized as `item: None`
/// `DocumentSymbol`s: an accessor per component ([JLS §8.10.3]) and the
/// canonical constructor ([JLS §8.10.4], skipped when the body declares an
/// explicit full-form constructor of matching arity).
fn record_members(
    db: &RootDatabase,
    file_id: FileId,
    symbol: &DocumentSymbol,
    data: &ItemData,
) -> Vec<DocumentSymbol> {
    let ItemData::Record(record) = data else {
        return Vec::new();
    };
    let mut members = Vec::with_capacity(record.components.len() + 1);
    let tree = hir::hir_def::java::plugin::tree(db, file_id);
    let ctx = crate::symbols::range_ctx(db, file_id, tree.language);
    let component_tys: Arc<Vec<String>> = Arc::new(
        hir_ty::record_component_types(db, file_id, symbol.item.unwrap())
            .into_iter()
            .map(|ty| ty.display_simple(db).to_string())
            .collect(),
    );

    // §8.10.3: each component has a public accessor `component(): T`. For a
    // varargs component the accessor returns the array type `T[]` (§8.4.1).
    // An accessor explicitly declared by the body (same name, zero parameters)
    // replaces the implicit one ([§8.10.3]).
    for (index, component) in record.components.iter().enumerate() {
        let simple = component.name.as_str().to_owned();
        let declares_accessor = record.body.iter().any(|item| {
            matches!(tree.data(*item), ItemData::Method(method)
                if method.name.as_str() == simple && method.sig.params.is_empty())
        });
        if declares_accessor {
            continue;
        }
        let ret = if component.varargs {
            format!("{}[]", component_tys[index])
        } else {
            component_tys[index].clone()
        };
        let component_range = ctx
            .as_ref()
            .and_then(|(map, source)| {
                hir::hir_def::java::ranges::component_range(map, source, component)
            })
            .unwrap_or_default();
        members.push(DocumentSymbol {
            name: format!("{}.{simple}", symbol.name),
            kind: hir::SourceSymbolKind::Method,
            // The accessor's range and selection range both target its whole
            // component declaration (`int x`, not just the name) — the
            // "definition" of the member.
            range: component_range,
            name_range: component_range,
            display_name: format!("{simple}(): {ret}"),
            detail: None,
            item: None,
        });
    }

    // §8.10.4: a record has a canonical constructor whose parameters mirror
    // the components in order. An explicit full-form constructor of matching
    // arity replaces it ([§8.10.4]).
    let simple = symbol
        .name
        .rsplit('.')
        .next()
        .unwrap_or(&symbol.name)
        .to_owned();
    let declares_canonical = record.body.iter().any(|item| {
        matches!(tree.data(*item), ItemData::Method(method) if method.is_constructor()
            && method.sig.params.len() == record.components.len())
    });
    if !declares_canonical {
        let params = record
            .components
            .iter()
            .enumerate()
            .map(|(index, component)| {
                if component.varargs {
                    format!("{}...", component_tys[index])
                } else {
                    component_tys[index].clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        members.push(DocumentSymbol {
            name: format!("{}.{simple}", symbol.name),
            kind: hir::SourceSymbolKind::Method,
            range: symbol.range,
            name_range: symbol.name_range,
            display_name: format!("{simple}({params})"),
            detail: None,
            item: None,
        });
    }
    members
}

/// The synthesized package symbol of a Java file.
pub(super) fn package_symbol(db: &RootDatabase, file_id: FileId) -> DocumentSymbol {
    let tree = hir::hir_def::java::plugin::tree(db, file_id);
    let name = tree.package.as_ref().map_or_else(
        || "<default package>".to_owned(),
        |name| name.as_str().to_owned(),
    );
    let range = crate::symbols::range_ctx(db, file_id, tree.language)
        .and_then(|(map, source)| {
            tree.package_decls.last().and_then(|decl| {
                hir::hir_def::java::ranges::package_name_range(&map, &source, *decl)
            })
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

/// The client-facing name of a document symbol: the simple name (the last
/// `.`-segment, `$` kept per [JLS §3.8](https://docs.oracle.com/javase/specs/jls/se26/html/jls-3.html#jls-3.8)),
/// with the signature rendered inline — `name(params): ret` for a method,
/// `name: type` for a field. The package keeps its full package name.
fn symbol_display_name(
    db: &RootDatabase,
    file_id: FileId,
    symbol: &hir::SourceSymbol,
    top_level: bool,
) -> String {
    let simple = symbol.name.simple_name();
    if top_level {
        return simple.to_owned();
    }
    match symbol.kind {
        hir::SourceSymbolKind::Method => method_signature(db, file_id, symbol.item, simple, true),
        hir::SourceSymbolKind::Field => {
            format!(
                "{simple}: {}",
                crate::symbols::item_ty(db, file_id, symbol.item)
            )
        }
        _ => simple.to_owned(),
    }
}

/// The `detail` of a symbol for the LSP `detail` field. Top-level types show
/// their fully qualified name; nested types show their kind and simple name;
/// methods, fields, enum constants and the package show none — their signature
/// is already part of [`DocumentSymbol::display_name`].
fn symbol_detail(symbol: &hir::SourceSymbol, top_level: bool) -> Option<String> {
    let simple = symbol.name.simple_name();
    if top_level {
        return Some(symbol.name.as_str().to_owned());
    }
    match symbol.kind {
        hir::SourceSymbolKind::Method
        | hir::SourceSymbolKind::Field
        | hir::SourceSymbolKind::EnumConstant => None,
        kind => Some(format!("{} {simple}", kind.label())),
    }
}

/// The rendered signature of a method or constructor: `simple(params)` with an
/// optional `: ret` (skipped for constructors and when `include_return` is
/// false). A variable-arity parameter renders its element type plus the
/// ellipsis ([JLS §8.4.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.4.1)),
/// so `String... names` is `String...`.
pub(crate) fn method_signature(
    db: &RootDatabase,
    file_id: FileId,
    item: hir::hir_def::jvm::ids::ItemId,
    simple: &str,
    include_return: bool,
) -> String {
    let is_constructor = matches!(
        hir::hir_def::java::plugin::tree(db, file_id).data(item),
        ItemData::Method(method) if method.is_constructor()
    );
    let ret = if include_return && !is_constructor {
        format!(": {}", crate::symbols::item_ty(db, file_id, item))
    } else {
        String::new()
    };
    format!("{simple}({}){ret}", render_params(db, file_id, item))
}

/// The parameter types of a method or constructor, in declaration order,
/// simple-rendered and joined with `, `, with `...` appended to a
/// variable-arity parameter.
fn render_params(
    db: &RootDatabase,
    file_id: FileId,
    item: hir::hir_def::jvm::ids::ItemId,
) -> String {
    let tree = hir::hir_def::java::plugin::tree(db, file_id);
    let varargs = matches!(
        tree.data(item),
        ItemData::Method(method) if method.sig.params.last().is_some_and(|param| param.varargs)
    );
    let mut params = crate::symbols::method_params(db, file_id, item).to_vec();
    if varargs && let Some(last) = params.last_mut() {
        *last = format!("{}...", last);
    }
    params.join(", ")
}

/// The declared range of `item` in a Java file. `None` for an item the file no
/// longer declares: a rewritten file's tree can shrink under a stale id.
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
    let tree = hir::hir_def::java::plugin::tree(db, file_id);
    let (map, source) = crate::symbols::range_ctx(db, file_id, tree.language)?;
    hir::hir_def::java::ranges::item_range(&map, &source, &tree, item)
}
