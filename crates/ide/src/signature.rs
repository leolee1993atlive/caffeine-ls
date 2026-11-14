//! Signature help: the declared candidates of the invocation the offset sits
//! inside, with the parameter the offset is in as the active one.

use vfs::FileId;

use ide_db::RootDatabase;
use jvm_index::hir_def::jvm::ids::ItemId;

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
    // A constructor "returns" its own type — the inference layer records
    // Error for it, and no return is what the reader wants to see.
    if !matches!(method.ret.kind(db), hir::TyKind::Error) {
        label.push_str(&format!(": {}", method.ret.display_simple(db)));
    }

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
