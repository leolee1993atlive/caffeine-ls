pub use java_syntax as java;
pub use kotlin_syntax as kotlin;

mod diagnostics;
pub mod lang;
mod language;
mod parse;

pub use diagnostics::{DiagnosticCode, JavaDiagnosticCode, KotlinDiagnosticCode};
pub use language::LanguageKind;
pub use parse::{Parse, SourceFile, SyntaxError};
