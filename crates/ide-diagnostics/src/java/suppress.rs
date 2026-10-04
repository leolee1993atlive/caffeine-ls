//! Java `@SuppressWarnings` scopes ([JLS §9.6.4.5]).

use hir_expand::name::Name;
use hir_ty::TyDatabase;
use hir_ty::java::resolve::NameResolution;
use rowan::{SyntaxNode, TextRange};
use rustc_hash::FxHashSet;
use syntax::SourceFile;
use syntax::java::{Lang, SyntaxKind as J, translate_unicode_escapes};
use vfs::FileId;

use crate::lint::{LintKey, SuppressionScope};

/// Every `@SuppressWarnings` scope of `file_id`, in source order.
///
/// A warning at `range` is suppressed when some returned scope both contains
/// `range` and carries its key; [`is_suppressed`] performs that check.
pub(crate) fn suppression_scopes(db: &dyn TyDatabase, file_id: FileId) -> Vec<SuppressionScope> {
    let tree = hir::hir_def::java::plugin::tree(db, file_id);
    let Some((_map, source)) = hir_ty::range_ctx::range_ctx(db, file_id, tree.language) else {
        return Vec::new();
    };
    let SourceFile::Java(file) = &source else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect(
        db,
        file_id,
        &file.syntax_node,
        &FxHashSet::default(),
        &mut out,
    );
    out
}

/// Walks the tree carrying the keys in effect, extending them at each
/// *declaration* that names `@SuppressWarnings` and recording its range.
///
/// The scope owner is the declaration node itself (`METHOD_DECL`,
/// `FIELD_DECL`, `CLASS_DECL`, `LOCAL_VARIABLE_DECLARATION`, `PARAMETER`,
/// `ENUM_CONSTANT`, `MODULE_DECL`, ...), because §9.6.4.5 scopes a
/// suppression to "the annotated declaration **or any of its parts**" — its
/// whole source range. [`declaration_keys`] reads the annotations off it,
/// wherever the grammar put them.
fn collect(
    db: &dyn TyDatabase,
    file_id: FileId,
    node: &SyntaxNode<Lang>,
    inherited: &FxHashSet<LintKey>,
    out: &mut Vec<SuppressionScope>,
) {
    let mut keys = inherited.clone();
    let own = declaration_keys(db, file_id, node);
    if !own.is_empty() {
        keys.extend(own);
        out.push(SuppressionScope {
            range: node.text_range(),
            keys: keys.clone(),
        });
    }
    for child in node.children() {
        collect(db, file_id, &child, &keys, out);
    }
}

/// The keys the `@SuppressWarnings` annotations written on the declaration
/// `node` name — empty for a node that is not one.
///
/// A declaration's annotations are the `ANNOTATION` children of its
/// `MODIFIER_LIST` child ([`grammar::modifiers`]), which is how every
/// declaration but one is parsed. The exception is an *enum constant*
/// ([§8.9.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.9.1)),
/// whose grammar has no modifier list: its annotations are children of the
/// `ENUM_CONSTANT` node itself, and they scope the constant exactly as a
/// method's scope its own body.
fn declaration_keys(
    db: &dyn TyDatabase,
    file_id: FileId,
    node: &SyntaxNode<Lang>,
) -> FxHashSet<LintKey> {
    let mut out = FxHashSet::default();
    if let Some(modifier_list) = node
        .children()
        .find(|child| child.kind() == J::MODIFIER_LIST)
    {
        for annotation in modifier_list.children() {
            if annotation.kind() == J::ANNOTATION {
                annotation_keys(db, file_id, &annotation, &mut out);
            }
        }
    }
    if node.kind() == J::ENUM_CONSTANT {
        for annotation in node.children() {
            if annotation.kind() == J::ANNOTATION {
                annotation_keys(db, file_id, &annotation, &mut out);
            }
        }
    }
    out
}

/// Adds the keys one `@SuppressWarnings` annotation names. An annotation of
/// another type — including one that merely *spells* its name the same —
/// names nothing; unrecognized strings are dropped ([JLS §9.6.4.5]).
fn annotation_keys(
    db: &dyn TyDatabase,
    file_id: FileId,
    annotation: &SyntaxNode<Lang>,
    out: &mut FxHashSet<LintKey>,
) {
    // The annotation's written name is the first `QUALIFIED_NAME` of its
    // node — the same node the item tree lowers the name from
    // ([`hir_def::java::lower::walk::annotation_name_ref`]).
    let Some(name_node) = annotation
        .descendants()
        .find(|node| node.kind() == J::QUALIFIED_NAME)
    else {
        return;
    };
    if !is_suppress_warnings(db, file_id, &name_node) {
        return;
    }
    // The marker form names no warning, and `@SuppressWarnings` is not a
    // marker annotation anyway
    // ([§9.6.4](https://docs.oracle.com/javase/specs/jls/se26/html/jls-9.html#jls-9.6.4)).
    if !annotation
        .children()
        .any(|child| child.kind() == J::ANNOTATION_ARGUMENT_LIST)
    {
        return;
    }
    // §9.7.1 makes an element value an *expression*, so a key may be the value
    // of a constant variable rather than a literal
    // (`static final String K = "unchecked"; @SuppressWarnings(K)`), which
    // javac honours — and a concatenation names a key only as its *whole*
    // value (`"un" + "checked"` is `unchecked`, `"unchecked" + "X"` is not
    // one). The type layer reads each element value as the constant it is;
    // what it cannot read names nothing, and must not be guessed from the
    // source, which would let the second of those pass for the first.
    let Some(values) =
        hir_ty::java::annotation_value::suppress_warnings_values(db, file_id, annotation)
    else {
        return;
    };
    out.extend(
        values
            .iter()
            .flat_map(|value| LintKey::keys_of(value))
            .copied(),
    );
}

/// Whether a written annotation name denotes `java.lang.SuppressWarnings`
/// itself — the *symbol*, not a spelling ([JLS
/// §6.5.5.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.5.5.1)).
/// The name resolves in the context of the declaration it annotates, so a
/// `SuppressWarnings` type declared in the file's package, imported with that
/// name, or nested in an enclosing class is *that* annotation: it suppresses
/// nothing, as javac has it ([JLS §9.6.4.5]).
fn is_suppress_warnings(
    db: &dyn TyDatabase,
    file_id: FileId,
    name_node: &SyntaxNode<Lang>,
) -> bool {
    let name = Name::new(&translate_unicode_escapes(
        name_node.text().to_string().trim(),
    ));
    matches!(
        hir_ty::java::resolve::resolve_written_name(db, file_id, name_node, &name),
        NameResolution::Resolved(resolved) if resolved.as_str() == SUPPRESS_WARNINGS
    )
}

/// The fully qualified name of the annotation §9.6.4.5 gives its meaning to.
const SUPPRESS_WARNINGS: &str = "java.lang.SuppressWarnings";
