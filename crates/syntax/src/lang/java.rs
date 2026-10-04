//! Java (JLS) as the syntax layer sees it.

use rowan::{GreenNode, SyntaxNode};

use crate::{
    JavaDiagnosticCode, LanguageKind, Parse, SourceFile, SyntaxError,
    diagnostics::DiagnosticCode,
    java::{self, LexicalErrorKind, ParseErrorKind, SyntaxErrorKind},
};

pub(crate) struct Java;

pub(crate) static JAVA: Java = Java;

impl super::LanguageSyntax for Java {
    fn kinds(&self) -> &'static [LanguageKind] {
        &[LanguageKind::Java]
    }

    fn file_extensions(&self) -> &'static [(&'static str, LanguageKind)] {
        &[("java", LanguageKind::Java)]
    }

    fn language_id(&self) -> &'static str {
        "java"
    }

    fn name(&self) -> &'static str {
        "java"
    }

    fn parse(&self, _kind: LanguageKind, text: &str) -> Parse {
        let (green, errors) = java::SourceFile::parse(text).into();
        Parse {
            green: Some(green),
            errors: errors.into_iter().map(syntax_error).collect(),
        }
    }

    fn syntax_node(&self, _kind: LanguageKind, green: GreenNode) -> SourceFile {
        SourceFile::Java(java::SourceFile {
            syntax_node: SyntaxNode::new_root(green),
        })
    }

    fn is_indexable(&self, raw_kind: u16) -> bool {
        use crate::java::SyntaxKind as Kind;
        use rowan::Language as _;
        let kind = crate::java::Lang::kind_from_raw(rowan::SyntaxKind(raw_kind));
        matches!(
            kind,
            Kind::PACKAGE_DECL
                | Kind::IMPORT_DECL
                | Kind::CLASS_DECL
                | Kind::INTERFACE_DECL
                | Kind::ENUM_DECL
                | Kind::RECORD_DECL
                | Kind::ANNOTATION_TYPE_DECL
                | Kind::MODULE_DECL
                | Kind::METHOD_DECL
                | Kind::CONSTRUCTOR_DECL
                | Kind::COMPACT_CONSTRUCTOR_DECL
                | Kind::ANNOTATION_TYPE_ELEMENT_DECL
                | Kind::FIELD_DECL
                | Kind::VARIABLE_DECLARATOR
                | Kind::ENUM_CONSTANT
                | Kind::STATIC_INITIALIZER
                | Kind::INSTANCE_INITIALIZER
                | Kind::FORMAL_PARAMETER
                | Kind::SPREAD_PARAMETER
                | Kind::TYPE
                | Kind::ANNOTATION
                | Kind::MARKER_ANNOTATION
                | Kind::QUALIFIED_NAME
                | Kind::REQUIRES_DIRECTIVE
                | Kind::EXPORTS_DIRECTIVE
                | Kind::OPENS_DIRECTIVE
                | Kind::USES_DIRECTIVE
                | Kind::PROVIDES_DIRECTIVE
        )
    }
}

fn syntax_error(err: java::SyntaxError) -> SyntaxError {
    let code = syntax_code(&err.kind);
    let message = err.kind.desc();

    SyntaxError {
        message,
        range: err.range,
        code,
    }
}

/// The Java code for a lexical error kind, when the kind is structured
/// enough to carry one (free-text `Message` kinds have none).
fn syntax_code(kind: &SyntaxErrorKind) -> Option<DiagnosticCode> {
    let code = match kind {
        SyntaxErrorKind::Lexer(kind) => match kind {
            LexicalErrorKind::UnexpectedChar(_) => JavaDiagnosticCode::UnexpectedChar,
            LexicalErrorKind::UnterminatedString => JavaDiagnosticCode::UnterminatedString,
            LexicalErrorKind::UnterminatedComment => JavaDiagnosticCode::UnterminatedComment,
            LexicalErrorKind::InvalidChar => JavaDiagnosticCode::InvalidChar,
            LexicalErrorKind::IllegalTextBlockOpen => JavaDiagnosticCode::IllegalTextBlockOpen,
            LexicalErrorKind::UnterminatedTextBlock => JavaDiagnosticCode::UnterminatedTextBlock,
            LexicalErrorKind::InvalidNumber => JavaDiagnosticCode::InvalidNumber,
            LexicalErrorKind::InvalidUnicodeEscape => JavaDiagnosticCode::InvalidUnicodeEscape,
            LexicalErrorKind::UnterminatedChar => JavaDiagnosticCode::UnterminatedChar,
            LexicalErrorKind::InvalidEscapeSequence => JavaDiagnosticCode::InvalidEscapeSequence,
            LexicalErrorKind::UnterminatedTemplate => JavaDiagnosticCode::UnterminatedTemplate,
        },
        SyntaxErrorKind::Parser(kind) => match kind {
            ParseErrorKind::ExpectedToken { .. } => JavaDiagnosticCode::ExpectedToken,
            ParseErrorKind::ExpectedContextualKeyword { .. } => JavaDiagnosticCode::ExpectedKeyword,
            ParseErrorKind::ExpectedConstruct(_) => JavaDiagnosticCode::ExpectedConstruct,
            ParseErrorKind::NotAStatement => JavaDiagnosticCode::NotAStatement,
            ParseErrorKind::Message(_) => return None,
        },
    };
    Some(DiagnosticCode::Java(code))
}
