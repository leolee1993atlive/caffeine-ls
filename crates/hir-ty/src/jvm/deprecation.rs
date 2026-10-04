//! Deprecation of a declaration
//! ([JLS §9.6.4.6](https://docs.oracle.com/javase/specs/jls/se26/html/jls-9.html#jls-9.6.4.6)).
//!
//! The detection that reads a Java or Kotlin declaration stays in that
//! language. This is the shared answer both report.

/// How a declaration is deprecated ([JLS §9.6.4.6]): a plain `@Deprecated`, or
/// `@Deprecated(forRemoval = true)` — *terminally* deprecated, whose use is a
/// warning javac reports by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deprecation {
    Ordinary,
    Terminal,
}
