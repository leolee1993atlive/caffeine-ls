//! The language-neutral snapshot surface of `hir-def`.
//!
//! `hir-def`'s tests render a lowered file as a stable, human-readable text so
//! a lowering change shows up as a reviewable diff. The rendering of a
//! *language's own* declaration model lives with that language
//! ([`crate::java::pretty`], [`crate::kotlin::pretty`]); this module renders
//! whichever model the facade holds.

use hir_expand::ast_id_map::AstIdMap;
use syntax::SourceFile;

use crate::item_tree::LoweredFile;

/// The stable, human-readable rendering of a lowered file: its declaration
/// model plus the source ranges resolved from the current syntax tree.
pub fn pretty_print(lowered: &LoweredFile, map: &AstIdMap, source: &SourceFile) -> String {
    lowered.items.declared().render(map, source)
}

/// The stable, human-readable rendering of a lowered file's bodies.
pub fn pretty_body(lowered: &LoweredFile) -> String {
    lowered.items.declared().pretty_body(&lowered.bodies)
}
