//! Java-side type construction.
//!
//! The *type model* lives in [`crate::ty`] — it is shared with Kotlin, which
//! interns, compares and displays types through the same machinery. This module
//! is the Java half: how a Java source reference becomes a [`Ty`]
//! ([`ty_from_source`]). Capture conversion and library signature lowering
//! live in [`crate::jvm`].
//!
//! It re-exports the model so that Java-side code keeps addressing it through
//! `crate::java::ty`.

use classfile::stub::TypeRef;
use hir_expand::name::Name;

use crate::jvm::db::TyDatabase;
pub use crate::jvm::ty::{boxed_type, numeric_promotion, primitive_name, unboxed_primitive};
pub use crate::ty::*;

/// Lowers a source [`TypeRef<Name>`] without name resolution (names kept
/// verbatim).
pub fn ty_from_source(db: &dyn TyDatabase, tyref: &TypeRef<Name>) -> Ty {
    crate::jvm::resolve::ty_from_type_ref(db, tyref, &mut |n| n.clone(), &mut |n| {
        TypeVarScope::Unnamed { name: n.clone() }
    })
}
