use vfs::FileId;

use crate::{LanguageKind, SourceDatabase};
use syntax::Parse;

/// Parse trees are large (a whole file's syntax). `lru = 512` bounds the
/// retained trees to roughly the open files plus one background chunk; the
/// least-recently-used are evicted at the next revision and re-parsed on
/// demand.
#[salsa::tracked(returns(ref), lru = 512)]
fn parse_query(db: &dyn SourceDatabase, file_id: FileId, language: LanguageKind) -> Parse {
    let text = db.file_text(file_id).text(db);
    syntax::SourceFile::parse(language, text)
}

pub fn parse(db: &dyn SourceDatabase, file_id: FileId, language: LanguageKind) -> &Parse {
    parse_query(db, file_id, language)
}
