//! Completion at an offset of a file: the members a `receiver.` position can
//! name. The Java answers come from the same member sets hover and
//! goto-definition resolve with; a Kotlin file answers nothing until its type
//! layer records member sets.

use rowan::TextSize;
use vfs::FileId;

use ide_db::RootDatabase;

/// One completion candidate: a member a dot position can name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// The member's name — what inserting writes.
    pub label: String,
    /// The kind of member, for the client's icon and filter.
    pub kind: CompletionItemKind,
    /// The member's rendered signature — the same rendering hover shows.
    pub detail: String,
}

/// The kind of a [`CompletionItem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionItemKind {
    Method,
    Field,
}

/// The completion at `offset`: the members the dot before it can name, empty
/// for an offset that is no dot position (or a receiver nothing infers).
pub fn completion(db: &RootDatabase, file: FileId, offset: TextSize) -> Vec<CompletionItem> {
    if matches!(
        ide_db::base_db::file_language_kind(db, file),
        Some(ide_db::base_db::LanguageKind::Java)
    ) {
        crate::nav::java::completion(db, file, offset)
    } else {
        Vec::new()
    }
}
