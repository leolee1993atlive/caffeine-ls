# One-shot patch: highlight read/write kinds + context templates.
# Run from caffeine-ls root.
import io

p = 'crates/ide/src/nav/java.rs'
s = io.open(p, encoding='utf-8').read()

# --- 1. highlight: write positions + kind ---
old = '''/// The document highlight of the symbol at `offset`: every position of this
/// file that writes or reads the same declaration — the declaration's own
/// token included. The LSP kind is left to the default (text): the sweep
/// behind [`references`] does not distinguish reads from writes yet.
pub(crate) fn document_highlight(
    db: &RootDatabase,
    file: FileId,
    offset: TextSize,
) -> Vec<TextRange> {
    references(db, file, offset, true)
        .into_iter()
        .filter(|hit| hit.file == file)
        .map(|hit| hit.range)
        .collect()
}'''
assert old in s, "highlight fn"
new = '''/// The document highlight of the symbol at `offset`: every position of this
/// file that writes or reads the same declaration — the declaration's own
/// token included — each tagged with its kind. A position is a *write* when
/// it is the declaration itself or the left-hand side of an assignment
/// ([JLS §15.26]), else a *read*.
pub(crate) fn document_highlight(
    db: &RootDatabase,
    file: FileId,
    offset: TextSize,
) -> Vec<(TextRange, bool)> {
    let hits: Vec<TextRange> = references(db, file, offset, true)
        .into_iter()
        .filter(|hit| hit.file == file)
        .map(|hit| hit.range)
        .collect();
    let writes = write_positions(db, file);
    hits.into_iter()
        .map(|range| {
            let is_write = writes.contains(&range);
            (range, is_write)
        })
        .collect()
}

/// The ranges of this file's positions that *write*: every assignment's
/// left-hand side, across every body of the file (methods, initializers,
/// nested classes — [`all_items`] reaches them all).
fn write_positions(db: &RootDatabase, file: FileId) -> FxHashSet<TextRange> {
    let tree = jvm_index::hir_def::java::plugin::tree(db, file);
    let mut out = FxHashSet::default();
    for (range, item) in all_items(db, file, &tree) {
        let _ = range;
        let Some(body) = hir::body_types(db, file, item) else {
            continue;
        };
        for (id, data) in body.exprs.iter() {
            if let ExprData::Assign { lhs, .. } = data
                && let Some(lhs_range) = body.expr_range(*lhs)
            {
                out.insert(lhs_range);
            }
        }
    }
    out
}'''
assert old in s
s = s.replace(old, new)

io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('highlight kinds done')
