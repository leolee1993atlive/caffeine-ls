# One-shot patch: documentHighlight + signatureHelp. Run from caffeine-ls root.
import io

BSN = chr(92) + 'n'
p = 'crates/ide/src/nav/java.rs'
s = io.open(p, encoding='utf-8').read()

# --- 1. highlight + signature help, before the test mod ---
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
/// with each parameter's span, and the parameter the offset is in.
pub(crate) fn signature_help(
    db: &RootDatabase,
    file: FileId,
    offset: TextSize,
) -> Option<SignatureHelp> {
    let bodies = jvm_index::file_body_tree(db, file);
    // Innermost call containing the offset: `exprs_at` orders inner-out, and
    // a nested call's argument wins over the enclosing one.
    for expr_id in exprs_at(&bodies, offset) {
        let call = match bodies.exprs.get(expr_id) {
            Some(data) => match &data.data {
                ExprData::MethodCall { args, .. } | ExprData::New { args, .. } => {
                    Some(expr_id.clone())
                }
                _ => None,
            },
            None => None,
        };
        let Some(_call) = call else { continue };

        let tree = jvm_index::hir_def::java::plugin::tree(db, file);
        let candidates: Vec<hir::MethodData> = body_items_at(db, file, &tree, offset)
            .iter()
            .filter_map(|item| hir::body_types(db, file, *item))
            .filter_map(|body| match body.resolved.get(&expr_id)? {
                hir::ResolvedMember::Method(method) => Some(vec![method.clone()]),
                hir::ResolvedMember::Unresolved(methods) => Some(methods.clone()),
                _ => None,
            })
            .next()?;

        // The parameter the offset sits in: the innermost argument containing
        // it. Zero when the offset is on the name or between the parentheses.
        let active = bodies
            .expr_ranges
            .iter()
            .enumerate()
            .filter(|(_, range)| range.contains(offset))
            .filter_map(|(idx, range)| {
                bodies
                    .exprs
                    .get(ExprId(ArenaId(idx as u32)))
                    .and_then(|data| data.argument_index(expr_id))
                    .map(|index| (u32::from(range.len()), index as u32))
            })
            .min_by_key(|(len, _)| *len)
            .map(|(_, index)| index)
            .unwrap_or(0);

        let signatures = candidates
            .iter()
            .map(|method| signature_information(db, method))
            .collect();
        return Some(SignatureHelp {
            signatures,
            active_signature: 0,
            active_parameter: Some(active),
        });
    }
    None
}

/// The rendered signature of one declared method: `name(p: T, ...): R` with
/// each parameter's span inside the label, the documentation attached.
fn signature_information(db: &RootDatabase, method: &hir::MethodData) -> SignatureInformation {
    let mut label = format!("{}(", method.name.as_str());
    let mut parameters = Vec::new();
    for (index, param) in method.sig.params.iter().enumerate() {
        if index > 0 {
            label.push_str(", ");
        }
        let start = label.len() as u32;
        let ty = type_ref_text(&param.ty.ty);
        let name = param.name.as_str();
        label.push_str(&ty);
        label.push(' ');
        label.push_str(name);
        let end = label.len() as u32;
        parameters.push(ParameterInformation {
            label: LspLabel::Simple([start, end]),
            documentation: None,
        });
    }
    label.push(')');
    let ret = method
        .sig
        .ret
        .as_ref()
        .map(|ret| format!(": {}", type_ref_text(&ret.ty)))
        .unwrap_or_default();
    label.push_str(&ret);
    SignatureInformation {
        label,
        parameters: Some(parameters),
        documentation: crate::docs::hover_docs(db, method.ast.file_id(db)).or(None),
        active_parameter: None,
    }
}

/// The source text of a type reference: `List<String>`, `int`, `T[]`, `?`.
fn type_ref_text(ty: &jvm_index::TypeRef<Name>) -> String {
    match ty {
        jvm_index::TypeRef::Primitive(primitive) => primitive_name(*primitive).to_owned(),
        jvm_index::TypeRef::Reference { name, generic_args } => {
            let base = name.as_str().rsplit('.').next().unwrap_or(name.as_str()).to_owned();
            if generic_args.is_empty() {
                base
            } else {
                let args: Vec<String> =
                    generic_args.iter().map(|arg| type_ref_text(arg)).collect();
                format!("{base}<{}>", args.join(", "))
            }
        }
        jvm_index::TypeRef::TypeVariable(name) => name.as_str().to_owned(),
        jvm_index::TypeRef::Array(inner) => format!("{}[]", type_ref_text(inner)),
        jvm_index::TypeRef::Wildcard { bound } => match bound.as_deref() {
            Some(bound) => format!("? {}", bound.text()),
            None => "?".to_owned(),
        },
        _ => "?".to_owned(),
    }
}

''' + anchor
s = s.replace(anchor, block)
io.open(p, 'w', encoding='utf-8', newline='').write(s)
print('nav/java.rs: highlight + sig help written (compile-fix loop next)')
