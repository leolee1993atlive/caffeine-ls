# One-shot patch: signature.rs module + wiring. Run from caffeine-ls root.
import io

# --- 1. the signature module ---
module = '''//! Signature help: the declared candidates of the invocation the offset sits
//! inside, with the parameter the offset is in as the active one.

use vfs::FileId;

use ide_db::RootDatabase;

/// The signature help of a position, as the LSP `textDocument/signatureHelp`
/// result: the candidates in declaration order, one of them active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: u32,
    pub active_parameter: Option<u32>,
}

/// One candidate: the rendered `name(p: T, ...): R` label, each parameter's
/// text (a substring of the label, the client-side highlight form), and the
/// declaration's documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInformation {
    pub label: String,
    pub parameters: Vec<ParameterInformation>,
    pub documentation: Option<String>,
}

/// One parameter of a signature: its text as a substring of the label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterInformation {
    pub label: String,
}

/// The signature of one declared candidate: parameters render as `T name`
/// with the source names a loaded declaration writes, and the return type
/// trails after the closing parenthesis. The documentation is the
/// declaration's own, when the declaration is a source one.
pub(crate) fn signature_information(
    db: &RootDatabase,
    method: &hir::MethodData,
    decl_item: Option<ItemId>,
) -> SignatureInformation {
    let mut label = format!("{}(", method.name);
    let mut parameters = Vec::new();
    for (index, ty) in method.params.iter().enumerate() {
        if index > 0 {
            label.push_str(", ");
        }
        let ty_text = ty.display_simple(db).to_string();
        let name = method
            .param_names
            .as_ref()
            .and_then(|names| names.get(index))
            .cloned()
            .unwrap_or_else(|| format!("arg{index}"));
        let parameter = format!("{ty_text} {name}");
        parameters.push(ParameterInformation { label: parameter });
        label.push_str(&parameters.last().unwrap().label);
    }
    label.push(')');
    label.push_str(&format!(": {}", method.ret.display_simple(db)));

    let documentation = method
        .owner_file
        .zip(decl_item)
        .and_then(|(file, item)| crate::docs::hover_docs(db, file, item));
    SignatureInformation {
        label,
        parameters,
        documentation,
    }
}
'''
io.open('crates/ide/src/signature.rs', 'w', encoding='utf-8', newline='').write(module)
print('signature.rs written')

# --- 2. lib.rs: module + Analysis methods ---
p = 'crates/ide/src/lib.rs'
s = io.open(p, encoding='utf-8').read()
old = '''pub mod nav;
pub mod symbols;'''
assert old in s, "mod list"
s = s.replace(old, '''pub mod nav;
pub mod signature;
pub mod symbols;''')

old = '''    /// The rename of the symbol at `offset` — every position the workspace
    /// writes its name, or `Err` explaining why the position or the new name
    /// cannot rename. Serves the LSP `textDocument/rename` request.
    pub fn rename('''
assert old in s, "analysis anchor"
addition = '''    /// The document highlight of the symbol at `offset` — every position of
    /// the file that names the same declaration. Serves the LSP
    /// `textDocument/documentHighlight` request.
    pub fn document_highlight(
        &self,
        file_id: FileId,
        offset: rowan::TextSize,
    ) -> Cancellable<Vec<rowan::TextRange>> {
        self.with_db(|db| nav::java::document_highlight(db, file_id, offset))
    }

    /// The signature help at `offset` — the candidates of the invocation the
    /// offset sits inside, or `None` when it is not inside one. Serves the
    /// LSP `textDocument/signatureHelp` request.
    pub fn signature_help(
        &self,
        file_id: FileId,
        offset: rowan::TextSize,
    ) -> Cancellable<Option<signature::SignatureHelp>> {
        self.with_db(|db| nav::java::signature_help(db, file_id, offset))
    }

''' + old
s = s.replace(old, addition)
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('lib.rs wired')

# --- 3. nav/java.rs: highlight + signature_help ---
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
/// — the selected overload, or every applicable one of a tie ([JLS
/// §15.12.2.5]) — with the parameter the offset sits in as the active one.
pub(crate) fn signature_help(
    db: &RootDatabase,
    file: FileId,
    offset: TextSize,
) -> Option<crate::signature::SignatureHelp> {
    let bodies = jvm_index::file_body_tree(db, file);
    // Innermost call containing the offset: `exprs_at` orders inner-out, and
    // a nested call's argument wins over the enclosing one.
    for expr_id in exprs_at(&bodies, offset) {
        if !matches!(
            bodies.expr(expr_id),
            ExprData::MethodCall { .. } | ExprData::New { .. }
        ) {
            continue;
        }
        // The parameter the offset sits in: the first argument containing it,
        // zero on the name or between the parentheses.
        let active = args_containing(&bodies, expr_id, offset)
            .into_iter()
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
                crate::signature::signature_information(db, &method, decl_item)
            })
            .collect();
        return Some(crate::signature::SignatureHelp {
            signatures,
            active_signature: 0,
            active_parameter: Some(active),
        });
    }
    None
}

/// The indexes of the call's arguments whose range contains `offset`, in
/// argument order — the active-parameter candidates.
fn args_containing(bodies: &BodyTree, call: ExprId, offset: TextSize) -> Vec<u32> {
    match bodies.expr(call) {
        ExprData::MethodCall { args, .. } | ExprData::New { args, .. } => args
            .iter()
            .enumerate()
            .filter(|(_, arg)| {
                bodies
                    .expr_ranges
                    .get(arg.0 .0 as usize)
                    .is_some_and(|range| range.contains(offset))
            })
            .map(|(index, _)| index as u32)
            .collect(),
        _ => Vec::new(),
    }
}

''' + anchor
s = s.replace(anchor, block)
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('nav/java.rs done')

# --- 4. handlers + dispatch + capabilities ---
p = 'crates/caffeine-ls/src/handlers/request.rs'
s = io.open(p, encoding='utf-8').read()
old = '''/// `textDocument/completion`: the members a `receiver.` position can name —'''
assert old in s, "handler anchor"
handlers = '''/// `textDocument/documentHighlight`: every position of the file that names
/// the symbol at the position.
pub fn on_document_highlight(
    state: GlobalStateSnapshot,
    params: DocumentHighlightParams,
) -> anyhow::Result<Option<Vec<DocumentHighlight>>> {
    let pos = params.text_document_position_params;
    tracing::info!(uri = ?pos.text_document.uri, "request document highlight");
    let Some(file_id) = state.url_to_file_id(&pos.text_document.uri)? else {
        return Ok(None);
    };
    let line_index = state.file_line_index(file_id)?;
    let offset = crate::lsp::from_proto::offset(&line_index, pos.position)?;
    let highlights = state.analysis.document_highlight(file_id, offset)?;
    Ok(Some(
        highlights
            .into_iter()
            .map(|range| DocumentHighlight {
                range: to_proto::range(&line_index, range),
                kind: None,
            })
            .collect(),
    ))
}

/// `textDocument/signatureHelp`: the candidates of the invocation the
/// position sits inside, the parameter under the cursor active.
pub fn on_signature_help(
    state: GlobalStateSnapshot,
    params: SignatureHelpParams,
) -> anyhow::Result<Option<lsp_types::SignatureHelp>> {
    let pos = params.text_document_position_params;
    tracing::info!(uri = ?pos.text_document.uri, "request signature help");
    let Some(file_id) = state.url_to_file_id(&pos.text_document.uri)? else {
        return Ok(None);
    };
    let line_index = state.file_line_index(file_id)?;
    let offset = crate::lsp::from_proto::offset(&line_index, pos.position)?;
    let Some(help) = state.analysis.signature_help(file_id, offset)? else {
        return Ok(None);
    };
    Ok(Some(lsp_types::SignatureHelp {
        signatures: help
            .signatures
            .into_iter()
            .map(|signature| SignatureInformation {
                label: signature.label,
                parameters: Some(
                    signature
                        .parameters
                        .into_iter()
                        .map(|parameter| ParameterInformation {
                            label: lsp_types::ParameterInformationLabel::String(parameter.label),
                            documentation: None,
                        })
                        .collect(),
                ),
                documentation: signature.documentation.map(|value| {
                    lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent {
                        kind: lsp_types::MarkupKind::Markdown,
                        value,
                    })
                }),
                active_parameter: None,
            })
            .collect(),
        active_signature: Some(help.active_signature),
        active_parameter: help.active_parameter,
    }))
}

''' + old
s = s.replace(old, handlers)
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('handlers added')

# --- 5. capabilities ---
p = 'crates/caffeine-ls/src/lsp/capabilities.rs'
s = io.open(p, encoding='utf-8').read()
old = '''        rename_provider: Some(true.into()),'''
assert old in s, "caps anchor"
s = s.replace(old, '''        rename_provider: Some(true.into()),
        document_highlight_provider: Some(true.into()),
        signature_help_provider: Some(
            SignatureHelpOptions {
                trigger_characters: Some(vec![",".to_string()]),
                ..Default::default()
            }
            .into(),
        ),''')
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('capabilities added')

# --- 6. dispatch ---
p = 'crates/caffeine-ls/src/main_loop.rs'
s = io.open(p, encoding='utf-8').read()
old = '''            .on_async::<RenameRequest>(handlers::on_rename)'''
assert old in s, "dispatch anchor"
s = s.replace(old, '''            .on_async::<RenameRequest>(handlers::on_rename)
            .on_async::<DocumentHighlightRequest>(handlers::on_document_highlight)
            .on_async::<SignatureHelpRequest>(handlers::on_signature_help)''')
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('dispatch added')
