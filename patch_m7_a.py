# One-shot patch: documentHighlight + signatureHelp (corrected). Run from caffeine-ls root.
import io

p = 'crates/ide/src/nav/java.rs'
s = io.open(p, encoding='utf-8').read()

anchor = '#[cfg(test)]\nmod completion_tests'
assert anchor in s, "test mod anchor"
block = '''// ---------------------------------------------------------------------------
// Highlight and signature help
//

/// The document highlight of the symbol at `offset`: every position of this
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
}

/// The signature help at `offset`: when the offset sits inside a method
/// invocation or an instance creation, the invocation's declared candidates
/// — the selected overload, or every applicable one of a tie ([JLS §15.12.2.5])
/// — with the parameter the offset is in as the active one.
pub(crate) fn signature_help(
    db: &RootDatabase,
    file: FileId,
    offset: TextSize,
) -> Option<crate::signature::SignatureHelp> {
    let bodies = jvm_index::file_body_tree(db, file);
    // Innermost call containing the offset: `exprs_at` orders inner-out, and a
    // nested call's argument wins over the enclosing one.
    for expr_id in exprs_at(&bodies, offset) {
        match bodies.expr(expr_id) {
            ExprData::MethodCall { args, .. } | ExprData::New { args, .. } => {
                // The parameter the offset sits in: the innermost argument
                // containing it; zero on the name or between the parentheses.
                let active = args
                    .iter()
                    .enumerate()
                    .filter(|(_, arg)| {
                        bodies
                            .expr_ranges
                            .get(arg.0 .0 as usize)
                            .is_some_and(|range| range.contains(offset))
                    })
                    .map(|(index, _)| index as u32)
                    .next()
                    .unwrap_or(0);

                let tree = jvm_index::hir_def::java::plugin::tree(db, file);
                let candidates: Vec<(hir::MethodData, Option<ItemId>)> =
                    body_items_at(db, file, &tree, offset)
                        .iter()
                        .filter_map(|item| {
                            let body = hir::body_types(db, file, *item)?;
                            let resolved = body.resolved.get(&expr_id)?;
                            match resolved {
                                hir::ResolvedMember::Method(method) => {
                                    Some(vec![(method.clone(), Some(*item))])
                                }
                                hir::ResolvedMember::Unresolved(methods) => Some(
                                    methods.iter().map(|m| (m.clone(), Some(*item))).collect(),
                                ),
                                _ => None,
                            }
                        })
                        .next()?;

                let signatures = candidates
                    .into_iter()
                    .map(|(method, decl_item)| {
                        crate::signature::signature_information(db, file, method, decl_item)
                    })
                    .collect();
                return Some(crate::signature::SignatureHelp {
                    signatures,
                    active_signature: 0,
                    active_parameter: Some(active),
                });
            }
            _ => continue,
        }
    }
    None
}

''' + anchor
s = s.replace(anchor, block)
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('nav/java.rs done')
