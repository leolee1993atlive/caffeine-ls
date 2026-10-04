//! Source symbol lookups over the HIR: per-file document symbols and
//! workspace-wide symbol search, plus direct access to the HIR type layer.
//!
//! These are the IDE-facing wrappers over the classpath-scoped source symbol
//! index of `hir` (see [`hir::source_set_symbols`]). All queries are salsa
//! memoized; the `ide::Analysis` methods below funnel them through the
//! cancellation boundary.

/// The `(map, source)` pair the on-demand range helpers resolve against, for
/// a file whose language is known.
pub(crate) fn range_ctx(
    db: &RootDatabase,
    file: FileId,
    language: ide_db::base_db::LanguageKind,
) -> Option<(hir_expand::ast_id_map::AstIdMap, syntax::SourceFile)> {
    // A kind no language answers for is a file with no source root yet (opened
    // before the workspace loaded) or a non-JVM file: it has no parse to
    // resolve ranges against.
    crate::lang::ide(language)?;
    let parse = ide_db::base_db::parse(db, file, language);
    let source = parse.syntax_node(language);
    let map = hir::hir_def::db::ast_id_map(db, file, language).clone();
    Some((map, source))
}
use rowan::TextRange;
use rustc_hash::FxHashSet;
use triomphe::Arc;
use vfs::FileId;

use crate::RootDatabase;

pub(crate) type ItemId = hir::hir_def::jvm::ids::ItemId;

/// A source symbol as seen by the IDE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    /// The canonical qualified name ([JLS §6.7](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.7)).
    /// Kept fully qualified so the LSP layer can nest members under their
    /// enclosing type; the client-facing text is [`DocumentSymbol::display_name`].
    pub name: String,
    pub kind: hir::SourceSymbolKind,
    /// The source range of the declaration.
    pub range: TextRange,
    /// The source range of just the declared name (the identifier).
    pub name_range: TextRange,
    /// The name shown to the client: the *simple* name (the last `.`-segment,
    /// `$` kept as an identifier character per [JLS
    /// §3.8](https://docs.oracle.com/javase/specs/jls/se26/html/jls-3.html#jls-3.8)),
    /// plus the signature where javac/IntelliJ render it inline — `name(params): ret`
    /// for a method (with `...` on a variable-arity parameter), `name: type`
    /// for a field. The package symbol keeps its full package name.
    pub display_name: String,
    /// The signature shown alongside the name: the fully qualified name for a
    /// top-level type, `kind simple` for a nested type, `None` for methods,
    /// fields, enum constants and the package.
    pub detail: Option<String>,
    /// The lowered item, for later HIR queries (e.g. [`crate::Analysis::item_ty`]).
    /// `None` for synthesized symbols (the file's package).
    pub item: Option<hir::hir_def::jvm::ids::ItemId>,
}

/// A workspace symbol picker row: the three fields a client renders before
/// navigation plus the `(file, item)` identity carried in the row's `data`.
/// Deliberately range- and signature-free: `workspace/symbol` stays O(index);
/// the LSP layer resolves the location for the one row the client navigates
/// to via `workspaceSymbol/resolve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSymbolSummary {
    pub file: FileId,
    /// Client-facing name: the simple name for type rows (`Foo`), and
    /// `{EnclosingType}.{simple}` for member rows (`Foo.bar`) — the dotted
    /// member form keeps `Class.member` queries matching the row's label in
    /// clients that filter server results client-side (VS Code).
    pub name: String,
    pub kind: hir::SourceSymbolKind,
    /// Client-facing container: the package for top-level types and members
    /// (`com.example`); the enclosing type's FQN for nested types. `None` in
    /// the unnamed package.
    pub container_name: Option<String>,
    pub item: ItemId,
}

/// The declared symbols of a file, in declaration order, prefixed by a
/// synthesized symbol for the file's package.
///
/// A record ([JLS §8.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.10))
/// is expanded with its *implicit* members — a public accessor method per
/// component ([§8.10.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.10.3))
/// and its canonical constructor ([§8.10.4](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.10.4))
/// — so the outline shows what the language synthesizes. These are synthetic
/// (`item: None`) and carry no source ranges of their own: the accessor
/// ranges target its record component, and the canonical constructor's
/// ranges target the record declaration.
pub fn document_symbols(db: &RootDatabase, file_id: FileId) -> Vec<DocumentSymbol> {
    crate::lang::for_file(db, file_id)
        .map_or_else(Vec::new, |ide| ide.document_symbols(db, file_id))
}

/// The synthesized symbol for the file's package declaration, shown above its
/// top-level types. The unnamed package ([JLS §7.4.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-7.html#jls-7.4.2))
/// is rendered explicitly as `<default package>`.
fn package_symbol(db: &RootDatabase, file_id: FileId) -> DocumentSymbol {
    crate::lang::for_file(db, file_id).map_or_else(
        || default_package_symbol(db, file_id),
        |ide| ide.package_symbol(db, file_id),
    )
}

/// The synthesized package symbol of a file no language declares: the default
/// package, with no range.
pub(crate) fn default_package_symbol(_db: &RootDatabase, _file_id: FileId) -> DocumentSymbol {
    let name = "<default package>".to_owned();
    DocumentSymbol {
        name: name.clone(),
        display_name: name,
        kind: hir::SourceSymbolKind::Package,
        range: TextRange::default(),
        name_range: TextRange::default(),
        detail: None,
        item: None,
    }
}

/// Symbols whose simple name starts with `query`, or whose canonical name
/// (`pkg.Enclosing.simple`, JLS §6.7) contains it — case-insensitive;
/// prefix-preferred (simple-name prefix matches are included alongside FQN
/// substring matches). Dotted queries work: `Class.member` and FQNs like
/// `com.example.Foo` match the canonical name. Restricted to `files` when
/// given, else every registered source set; sorted by (canonical name, file,
/// item) and deduplicated, with an empty query returning everything (within
/// scope).
pub fn workspace_symbol_summaries(
    db: &RootDatabase,
    query: &str,
    files: Option<&[FileId]>,
) -> Vec<WorkspaceSymbolSummary> {
    let mut refs: Vec<hir::SourceSymbolRef> = Vec::new();
    match files {
        None => {
            // Index path — exactly the current collection logic, no filtering.
            for source_set in registered_source_sets(db) {
                let index = hir::source_set_symbols(db, source_set);
                if query.trim().is_empty() {
                    refs.extend(index.iter().cloned());
                } else {
                    refs.extend(index.lookup_simple(query));
                    refs.extend(index.lookup_fqn_substring(query));
                }
            }
        }
        Some(files) => {
            // Opened-files path (empty query by contract): per-file item trees
            // only, no source-set index scan. `contains` on the canonical
            // name, mirroring the index lookups (simple-name prefix ∪ FQN
            // substring).
            let query = query.to_lowercase();
            for &file in files {
                for symbol in hir::file_symbols(db, file).iter() {
                    if query.trim().is_empty()
                        || symbol.name.as_str().to_lowercase().contains(&query)
                    {
                        refs.push(hir::SourceSymbolRef {
                            file,
                            symbol: symbol.clone(),
                        });
                    }
                }
            }
        }
    }
    refs.sort_by_key(|reference| {
        (
            reference.symbol.name.as_str().to_owned(),
            reference.file,
            reference.symbol.item,
        )
    });
    refs.dedup();
    refs.into_iter()
        .map(|reference| {
            let fqn = reference.symbol.name.as_str().to_owned();
            let (name, container_name) = match reference.symbol.kind {
                // Members render as `{EnclosingType}.{simple}` (`Foo.bar`)
                // with the container reduced to the package, so dotted and
                // fully-qualified queries survive the client's local filter,
                // which fuzzy-matches the row's `name` and drops rows it
                // does not match.
                hir::SourceSymbolKind::Method
                | hir::SourceSymbolKind::Field
                | hir::SourceSymbolKind::EnumConstant => {
                    let (parent, simple) = fqn.rsplit_once('.').unwrap_or(("", fqn.as_str()));
                    let (container, enclosing) = parent
                        .rsplit_once('.')
                        .map_or((None, parent), |(pkg, ty)| (Some(pkg), ty));
                    (
                        format!("{}.{}", enclosing, simple),
                        container.map(str::to_owned),
                    )
                }
                _ => (
                    fqn.rsplit_once('.')
                        .map(|(_, simple)| simple.to_owned())
                        .unwrap_or_else(|| fqn.clone()),
                    fqn.rsplit_once('.').map(|(parent, _)| parent.to_owned()),
                ),
            };
            WorkspaceSymbolSummary {
                file: reference.file,
                name,
                kind: reference.symbol.kind,
                container_name,
                item: reference.symbol.item,
            }
        })
        .collect()
}

/// The declaration range of one lowered item, on demand — the
/// `workspaceSymbol/resolve` half of the flow. Returns `None` for a stale
/// `(file, item)` (file rewritten or deleted since the row was served).
pub fn source_symbol_range(db: &RootDatabase, file_id: FileId, item: u32) -> Option<TextRange> {
    let item = hir::hir_def::jvm::ids::ItemId(hir_expand::arena::ArenaId(item));
    crate::lang::for_file(db, file_id).and_then(|ide| ide.source_symbol_range(db, file_id, item))
}

/// The registered source sets of the project graph, in unspecified order.
fn registered_source_sets(db: &RootDatabase) -> Vec<hir::SourceSetId> {
    hir::project_graph(db)
        .map(|graph| graph.source_sets(db).keys().cloned().collect())
        .unwrap_or_default()
}

/// The declared type of an item — a field's type, a method's return type, or
/// the type of a class-like declaration — rendered from the HIR type layer
/// with the *simple* class name (the last `.`-segment).
pub fn item_ty(db: &RootDatabase, file_id: FileId, item: hir::hir_def::jvm::ids::ItemId) -> String {
    crate::lang::for_file(db, file_id)
        .map_or_else(String::new, |ide| ide.item_ty(db, file_id, item))
}

/// The parameter types of a method or constructor, in declaration order,
/// simple-rendered from the HIR type layer.
pub fn method_params(
    db: &RootDatabase,
    file_id: FileId,
    item: hir::hir_def::jvm::ids::ItemId,
) -> Arc<[String]> {
    crate::lang::for_file(db, file_id).map_or_else(
        || Arc::from(Vec::new()),
        |ide| ide.method_params(db, file_id, item),
    )
}
