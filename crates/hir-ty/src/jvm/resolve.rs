//! Classfile-shaped name and type lowering shared by every language.
//!
//! A library signature is already fully qualified. Source item trees stay in
//! the language that owns them; this module only dispatches to that language
//! for a source class's declared type parameters.

use classfile::stub::{TypeBound, TypeRef};
use hir_expand::name::Name;
use rustc_hash::FxHashMap;

use crate::jvm::db::TyDatabase;
use crate::ty::{BoundKind, Ty, TyKind, TypeVarScope, WildcardBound};

/// The classfile `Signature`-lowering context ([JVMS §4.7.9.1]): the class
/// whose signature is being lowered and, for a member signature, the member,
/// so every type variable in it is attributed to its declaring parameter
/// ([JLS §4.4], [§6.3]).
#[derive(Clone, Copy)]
pub struct LibrarySignature<'a> {
    pub owner: &'a Name,
    pub method: Option<(&'a Name, &'a [Name])>,
}

impl<'a> LibrarySignature<'a> {
    /// The signature of the class `owner` itself (its supertypes, its own
    /// type-parameter bounds, a field's type).
    pub fn class(owner: &'a Name) -> Self {
        Self {
            owner,
            method: None,
        }
    }

    /// The scope of the type variable `name` within this signature
    /// ([§6.4.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.4.1)):
    /// the member's own parameter when the member declares that name,
    /// otherwise the class's. A name declared by neither is an enclosing
    /// class's parameter, which this signature does not carry an argument for
    /// — it keeps the class identity so no binding of this class captures it.
    pub(crate) fn scope_of(&self, name: &Name) -> TypeVarScope {
        TypeVarScope::library(self.owner, self.method, name)
    }
}

/// The declaration a reference type denotes: its declaration for a *local*
/// class-like type ([JLS §14.3], [§6.7] — it has no canonical name to resolve),
/// otherwise the class [`hir::fqn_resolve`] finds for its canonical name
/// against `scope`'s classpath.
pub fn reference_class(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    ty: &Ty,
) -> Option<hir::Resolved> {
    let TyKind::Reference { name, local, .. } = ty.kind(db) else {
        return None;
    };
    match local {
        Some(class) => Some(hir::Resolved::Source(*class)),
        None => hir::fqn_resolve(db, scope, name.as_str()),
    }
}

/// The substitution instantiating the type parameters of a *classfile* class
/// declaration ([JVMS §4.7.9.1](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-4.html#jvms-4.7.9.1)):
/// each declared parameter, keyed by its declaring class's binary name
/// ([JLS §4.4], [§6.3]), bound to the receiver's argument at the same index.
pub fn library_class_binding(
    owner: &Name,
    params: &[Name],
    args: &[Ty],
) -> FxHashMap<TypeVarScope, Ty> {
    params
        .iter()
        .zip(args.iter().copied())
        .map(|(name, arg)| {
            (
                TypeVarScope::LibraryClass {
                    owner: owner.clone(),
                    name: name.clone(),
                },
                arg,
            )
        })
        .collect()
}

/// The canonical fully qualified name of a resolved class
/// ([JLS §6.7](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.7)),
/// or `None` for a source class (whose name is not a binary one — bindings
/// against it are keyed by its declaring item, not by name).
pub(crate) fn resolved_fqn(db: &dyn TyDatabase, resolved: &hir::Resolved) -> Option<Name> {
    match resolved {
        hir::Resolved::Library(class) => {
            let interner = &db.hir_state().interner;
            Some(Name::new(interner.resolve(&class.entry.fqn)))
        }
        // A facade's name is the one the compiler gives it: it is not a
        // *declaration*, so it has no binary name of its own either.
        hir::Resolved::Facade { fqn, .. } => Some(fqn.clone()),
        hir::Resolved::Source(_) => None,
    }
}

/// Lowers a [`TypeRef`] into a [`Ty`], mapping names with `name`. Reference
/// names are used verbatim (no resolution): use a language's own resolver for
/// source-side resolution, and this for library `TypeRef<Symbol>`s whose names
/// are already fully qualified.
pub fn ty_from_type_ref<N>(
    db: &dyn TyDatabase,
    tyref: &TypeRef<N>,
    name: &mut dyn FnMut(&N) -> Name,
    var: &mut dyn FnMut(&N) -> TypeVarScope,
) -> Ty {
    match tyref {
        TypeRef::Primitive(p) => Ty::primitive(db, *p),
        // `void` is a type ([JLS §4.3]) but not a primitive ([§4.2]), and the
        // model gives it its own kind.
        TypeRef::Void => Ty::void(db),
        TypeRef::Reference {
            name: n,
            generic_args,
        } => {
            let args = generic_args
                .iter()
                .map(|arg| ty_from_type_ref(db, arg, name, var))
                .collect();
            Ty::reference(db, name(n), args)
        }
        TypeRef::Wildcard { bound } => Ty::wildcard(
            db,
            bound.as_deref().map(|b| match b {
                TypeBound::Upper(t) => Box::new(WildcardBound {
                    kind: BoundKind::Upper,
                    ty: ty_from_type_ref(db, t, name, var),
                }),
                TypeBound::Lower(t) => Box::new(WildcardBound {
                    kind: BoundKind::Lower,
                    ty: ty_from_type_ref(db, t, name, var),
                }),
            }),
        ),
        TypeRef::TypeVariable(v) => Ty::type_var(db, var(v), Vec::new()),
        TypeRef::Array(inner) => Ty::array(db, ty_from_type_ref(db, inner, name, var)),
        TypeRef::Error => Ty::error(db),
        // Kotlin-only ([`TypeRef::Nullable`]): a Java source type and a
        // classfile descriptor are never nullable ([JLS §4.1]).
        TypeRef::Nullable(_) | TypeRef::DefinitelyNonNull(_) => {
            debug_assert!(false, "a Java type reference is never nullable");
            Ty::error(db)
        }
    }
}

/// Lowers a library [`TypeRef<Symbol>`] to a [`Ty`]. Library names are
/// already fully qualified, so only the interner lookup is needed.
pub fn ty_from_library(db: &dyn TyDatabase, tyref: &TypeRef<hir::Symbol>) -> Ty {
    let interner = &db.hir_state().interner;
    ty_from_type_ref(
        db,
        tyref,
        &mut |symbol| Name::new(interner.resolve(symbol)),
        // No declaring context: a classfile signature lowered without its
        // class cannot attribute its type variables to a parameter, so they
        // become unscoped — no declaration-scoped binding captures them.
        // Signatures that *do* carry type variables go through
        // [`ty_from_library_signature`].
        &mut |symbol| TypeVarScope::Unnamed {
            name: Name::new(interner.resolve(symbol)),
        },
    )
}

/// Lowers a classfile [`TypeRef<Symbol>`] of `ctx`'s `Signature` attribute
/// ([JVMS §4.7.9.1](https://docs.oracle.com/javase/specs/jvms/se26/html/jvms-4.html#jvms-4.7.9.1)),
/// attributing every type variable to its declaring parameter ([JLS §4.4],
/// [§6.3]) so a binding of that declaration instantiate exactly its own
/// variables — and never a same-named parameter of another declaration
/// ([§6.4.1]).
pub fn ty_from_library_signature(
    db: &dyn TyDatabase,
    tyref: &TypeRef<hir::Symbol>,
    ctx: &LibrarySignature<'_>,
) -> Ty {
    let interner = &db.hir_state().interner;
    ty_from_type_ref(
        db,
        tyref,
        &mut |symbol| Name::new(interner.resolve(symbol)),
        &mut |symbol| ctx.scope_of(&Name::new(interner.resolve(symbol))),
    )
}

/// The declared type parameters of the class named by `fqn`, as
/// `(declaring scope, resolved bounds)`. A library class is answered by
/// [`crate::lang::classfile`]; a source class by the language that declares it.
pub fn declared_type_param_scopes(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    fqn: &Name,
) -> Vec<(TypeVarScope, Vec<Ty>)> {
    let Some(resolved) = hir::fqn_resolve(db, scope, fqn.as_str()) else {
        return Vec::new();
    };
    match &resolved {
        hir::Resolved::Library(_) => crate::lang::classfile().type_param_bounds(db, &resolved),
        class => crate::lang::for_class(db, class)
            .map(|language| language.type_param_bounds(db, class))
            .unwrap_or_default(),
    }
}

/// The classfile `Signature` type parameters of `resolved`, or empty when it
/// is not a library class or carries no generic signature.
pub(crate) fn library_type_param_bounds(
    db: &dyn TyDatabase,
    resolved: &hir::Resolved,
) -> Vec<(TypeVarScope, Vec<Ty>)> {
    let hir::Resolved::Library(_) = resolved else {
        return Vec::new();
    };
    let Some(info) = hir::class_generic_info(db, resolved) else {
        return Vec::new();
    };
    let Some(owner) = resolved_fqn(db, resolved) else {
        return Vec::new();
    };
    let interner = &db.hir_state().interner;
    let names: Vec<Name> = info
        .type_params
        .iter()
        .map(|tp| Name::new(interner.resolve(&tp.name)))
        .collect();
    let ctx = LibrarySignature::class(&owner);
    info.type_params
        .iter()
        .zip(names.iter())
        .map(|(tp, name)| {
            (
                TypeVarScope::LibraryClass {
                    owner: owner.clone(),
                    name: name.clone(),
                },
                tp.bounds
                    .iter()
                    .map(|bound| ty_from_library_signature(db, bound, &ctx))
                    .collect(),
            )
        })
        .collect()
}
