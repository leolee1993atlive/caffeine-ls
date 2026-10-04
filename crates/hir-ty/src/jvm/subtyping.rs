//! Subtyping and the direct-supertype walk shared by every language.
//!
//! [`is_subtype`] implements the subtype relation of
//! [JLS §4.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10).
//! The per-scope results are memoized as tracked queries. A class's declared
//! supertypes are answered by the language that declares it, or by
//! [`crate::lang::classfile`] for a library class; this module does not read
//! a language's item tree.

use rustc_hash::FxHashSet;

use crate::jvm::db::{ScopeId, ScopeKind, TyDatabase};
use crate::ty::{BoundKind, Ty, TyData, TyKind, WildcardBound};

/// The direct supertypes of `ty`
/// ([JLS §4.10.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10.2),
/// [§4.10.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10.3)):
///
/// * for a class/interface, its direct superclass and interfaces as declared
///   in the classfile (erasure-style: tier-1 classfile data does not carry
///   type arguments);
/// * for an array type, `Object`, `Cloneable` and `Serializable`.
///
/// Types outside the resolvable hierarchy (source classes, type variables,
/// primitives) yield no supertypes.
pub fn supertypes(db: &dyn TyDatabase, scope: &jvm_index::ResolutionScope, ty: &Ty) -> Vec<Ty> {
    let scope = ScopeId::new(db, ScopeKind::from_scope(scope));
    supertypes_query(db, scope, ty.id)
}

/// Memoized per (scope, type). See [`supertypes`].
#[salsa::tracked(returns(clone))]
pub(crate) fn supertypes_query(db: &dyn TyDatabase, scope: ScopeId, ty: TyData) -> Vec<Ty> {
    supertypes_impl(db, &scope.kind(db).to_scope(), &Ty { id: ty })
}

/// The non-memoized form of [`supertypes`], usable with types that carry
/// inference variables ([`TyKind::InferenceVar`], JLS §18.1.1) — such types
/// must never reach the memoized tracked query, and the invocation type
/// inference ([§18.5.2]) needs the direct supertype walk of a parameterized
/// class whose type arguments are still being solved.
pub(crate) fn supertypes_impl(
    db: &dyn TyDatabase,
    scope: &jvm_index::ResolutionScope,
    ty: &Ty,
) -> Vec<Ty> {
    match ty.kind(db) {
        TyKind::Reference { .. } => {
            // §6.7: the supertypes of the type's *declaration* — a local
            // class's own item, or the class its canonical name resolves to.
            let Some(resolved) = crate::jvm::resolve::reference_class(db, scope, ty) else {
                return Vec::new();
            };
            match &resolved {
                jvm_index::Resolved::Library(_) => {
                    crate::lang::classfile().supertypes(db, scope, *ty)
                }
                jvm_index::Resolved::Source(_) | jvm_index::Resolved::Facade { .. } => {
                    crate::lang::for_class(db, &resolved)
                        .map(|language| {
                            language
                                .supertypes(db, scope, *ty)
                                .into_iter()
                                .map(|supertype| language.ty_to_jvm(db, supertype))
                                .collect()
                        })
                        .unwrap_or_default()
                }
            }
        }
        TyKind::Array(_) => vec![
            Ty::reference(db, "java.lang.Object", Vec::new()),
            Ty::reference(db, "java.lang.Cloneable", Vec::new()),
            Ty::reference(db, "java.io.Serializable", Vec::new()),
        ],
        _ => Vec::new(),
    }
}

/// Whether `sub` is a subtype of `sup`
/// ([JLS §4.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10)).
pub fn is_subtype(
    db: &dyn TyDatabase,
    scope: &jvm_index::ResolutionScope,
    sub: &Ty,
    sup: &Ty,
) -> bool {
    let scope = ScopeId::new(db, ScopeKind::from_scope(scope));
    is_subtype_query(db, scope, sub.id, sup.id)
}

/// Memoized per (scope, sub, sup). See [`is_subtype`].
#[salsa::tracked(returns(clone))]
pub(crate) fn is_subtype_query(
    db: &dyn TyDatabase,
    scope: ScopeId,
    sub: TyData,
    sup: TyData,
) -> bool {
    let sub = Ty { id: sub };
    let sup = Ty { id: sup };
    if sub == sup {
        return true;
    }
    // §4.10.2: reference-type reflexivity holds for a parameterized type
    // identical *by name* even when the two handles intern differently — the
    // recursion guard of §4.4 bound resolution truncates a self-referential
    // bound (`T extends Box<K,T>`) at different depths per resolver context.
    // A type variable is a subtype of itself and of its own parameterization.
    if sub.same_shape(db, &sup) {
        return true;
    }
    match (sub.kind(db), sup.kind(db)) {
        // §4.10.1: the primitive partial order is identity only (widening is
        // a conversion, not a subtyping).
        (TyKind::Primitive(_), TyKind::Primitive(_)) => false,
        // §4.10.3: S[] <: T[] iff S <: T.
        (TyKind::Array(_), TyKind::Array(_)) => match (sub.element(db), sup.element(db)) {
            (Some(s), Some(t)) => is_subtype_query(db, scope, s.id, t.id),
            _ => false,
        },
        // §4.10.3: T[] <: Object, Cloneable, Serializable.
        (TyKind::Array(_), TyKind::Reference { name, .. }) => matches!(
            name.as_str(),
            "java.lang.Object" | "java.lang.Cloneable" | "java.io.Serializable"
        ),
        // §4.10.2 with §5.1.10: a type variable with a *lower* bound `L` is a
        // subtype of every type `X` with `L <: X` — the capture of `? super
        // T` ranges over `{ X : T <: X }`, so a read typed `CAP` converts to
        // `Object` (and to every supertype of `L` above it) but never to a
        // type below `L`. The rule belongs in `type_var_subtype` (where the
        // target's *upper* bounds and the source's own bounds are walked);
        // reaching it from this arm for a non-variable target only when the
        // target is above the lower bound keeps the memoized query acyclic —
        // the capture variable's bound chain (`CAP :> L` plus `CAP <: Object`)
        // is what the JLS §4.10.2 subtyping of a capture variable states.
        //
        // [§4.10.2]/[§5.1.10] *write* direction: `S <: CAP` for a capture
        // `CAP` of `? super L` holds exactly when `S <: L` — §4.10.2 makes a
        // type variable a direct supertype of its lower bound (`L <: CAP`),
        // so every `S` below `L` is below `CAP` by transitivity, while
        // `Object <: CAP` can only hold when `Object <: L`. So the source
        // argument is checked against the wildcard's *lower bound*, not the
        // other way around: `Sub <: CAP(? super Base)` holds for `Sub extends
        // Base` (javac accepts `list.add(sub)`), and `Number <: CAP(? super
        // Integer)` does not (javac rejects `list.add(number)` with "Number
        // cannot be converted to CAP#1 ... super: Integer").
        (
            _,
            TyKind::TypeVar {
                lower: Some(lower), ..
            },
        ) => is_subtype_query(db, scope, sub.id, lower.id),
        (TyKind::TypeVar { .. }, TyKind::Reference { .. })
        | (TyKind::TypeVar { .. }, TyKind::TypeVar { .. }) => type_var_subtype(db, scope, sub, sup),
        (TyKind::Reference { .. }, TyKind::Reference { .. }) => {
            reference_subtype(db, scope, sub, sup)
        }
        // §4.10.2/§4.10.3: the null type is a subtype of every reference type
        // (class, interface, type variable, array and intersection).
        (TyKind::Null, TyKind::Reference { .. } | TyKind::Array(_) | TyKind::TypeVar { .. }) => {
            true
        }
        (TyKind::Null, TyKind::Intersection(members)) => members
            .iter()
            .all(|member| is_subtype_query(db, scope, sub.id, member.id)),
        // §4.10.2: `S <: A & B` iff `S <: A` and `S <: B`; `A & B <: T` iff
        // some member is a subtype of `T` (§4.9). Equal intersections are
        // already handled by the identity check above.
        (TyKind::Intersection(members), _) => members
            .iter()
            .any(|member| is_subtype_query(db, scope, member.id, sup.id)),
        (_, TyKind::Intersection(members)) => members
            .iter()
            .all(|member| is_subtype_query(db, scope, sub.id, member.id)),
        _ => false,
    }
}

/// Whether the type variable `sub` is a subtype of `sup`
/// ([JLS §4.10.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10.2)):
/// every type variable is a subtype of `Object`, and of whatever its declared
/// bounds ([§4.4](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.4))
/// are subtypes of, walking the bound chain (`<T extends U, U extends Number>`
/// gives `T <: Number`).
fn type_var_subtype(db: &dyn TyDatabase, scope: ScopeId, sub: Ty, sup: Ty) -> bool {
    if sup.is_object(db) {
        return true;
    }
    // §4.10.2 with §5.1.10: when the *target* is a capture variable with a
    // lower bound `L` (`? super L`), a source `S <: CAP` holds exactly when
    // `S <: L` — a type variable is a direct supertype of its lower bound, so
    // `L <: CAP` and subtyping transitivity carry every `S` below `L` below
    // `CAP`. `Object <: CAP` holds only when `Object <: L`, which is why a
    // bounded type variable (`W extends Base` into `? super Base`) is
    // accepted — `S <: L` is subtyping, not identity.
    if let TyKind::TypeVar {
        lower: Some(lower), ..
    } = sup.kind(db)
    {
        return is_subtype_query(db, scope, sub.id, lower.id);
    }
    let mut visited = FxHashSet::default();
    let mut stack = vec![sub];
    while let Some(current) = stack.pop() {
        // Also the visited key: a variable is identified by its declaring
        // parameter ([§4.4], [§6.3]), not by its bare name ([§6.4.1]).
        let TyKind::TypeVar {
            scope: var_scope,
            bounds,
            ..
        } = current.kind(db)
        else {
            continue;
        };
        // §4.10.2 with §5.1.10: a *source* capture variable `CAP` with lower
        // bound `L` is a subtype of every `X` with `L <: X` — the value it
        // holds is at least `L`, so a read typed by the `? super L` capture
        // converts upward. (A write of `S` into the `? super L` receiver is
        // `S <: CAP`, decided by the target-lower arm above.)
        if let TyKind::TypeVar {
            lower: Some(lower), ..
        } = current.kind(db)
            && is_subtype_query(db, scope, lower.id, sup.id)
        {
            return true;
        }
        if !visited.insert(var_scope.clone()) {
            continue;
        }
        for bound in bounds {
            // §4.10.2: a bound identical to the target *by name* — a
            // self-referential bound (`T extends Box<K,T>`) is resolved to
            // differently-deep handles per resolver context — satisfies the
            // subtyping directly.
            if bound == &sup || bound.same_shape(db, &sup) {
                return true;
            }
            match bound.kind(db) {
                // A reference bound reaches `sup` through its supertype closure.
                TyKind::Reference { .. } => {
                    if is_subtype_query(db, scope, bound.id, sup.id) {
                        return true;
                    }
                }
                // A type-var bound keeps unwinding the bound chain.
                TyKind::TypeVar { .. } => stack.push(*bound),
                _ => {}
            }
        }
    }
    false
}

/// Reference subtyping: identical erasure (§4.10.2) or membership in the
/// declared supertype closure.
fn reference_subtype(db: &dyn TyDatabase, scope: ScopeId, sub: Ty, sup: Ty) -> bool {
    let (sub_name, sub_args) = sub.as_reference(db).expect("reference kind");
    let (sup_name, sup_args) = sup.as_reference(db).expect("reference kind");
    if sub_name == sup_name {
        // Same erasure: the parameterized types are subtypes only when the
        // type arguments are compatible (§4.10.2: invariant except wildcards).
        return params_ok(db, scope, sub_args, sup_args);
    }

    // Otherwise walk the transitive closure of `sub`'s supertypes (§4.10.2).
    let mut stack = vec![sub];
    let mut visited = FxHashSet::default();
    while let Some(current) = stack.pop() {
        for parent in supertypes_query(db, scope, current.id) {
            if !visited.insert(parent) {
                continue;
            }
            if parent == sup {
                return true;
            }
            if let Some((parent_name, parent_args)) = parent.as_reference(db)
                && parent_name == sup_name
                && params_ok(db, scope, parent_args, sup_args)
            {
                return true;
            }
            stack.push(parent);
        }
    }
    false
}

/// Whether `sub_args` are subtype-compatible with `sup_args` (§4.10.2).
fn params_ok(db: &dyn TyDatabase, scope: ScopeId, sub_args: &[Ty], sup_args: &[Ty]) -> bool {
    if sup_args.is_empty() {
        // A raw or non-generic super accepts any instantiation (§4.8).
        return true;
    }
    if sub_args.is_empty() {
        // A raw type is not a subtype of a parameterized type (§4.8).
        return false;
    }
    sub_args.len() == sup_args.len()
        && sub_args
            .iter()
            .zip(sup_args)
            .all(|(sub, sup)| arg_ok(db, scope, sub, sup))
}

fn arg_ok(db: &dyn TyDatabase, scope: ScopeId, sub: &Ty, sup: &Ty) -> bool {
    match (sub.kind(db), sup.kind(db)) {
        // `?` accepts any type argument (§4.5.1: `? extends T <= ?` and
        // `? super T <= ?`).
        (_, TyKind::Wildcard(None)) => true,
        // A wildcard against a bounded wildcard: the contains relation
        // (§4.5.1) drives parameterized subtyping between wildcard arguments
        // (§4.10.2).
        (TyKind::Wildcard(Some(sub_bound)), TyKind::Wildcard(Some(sup_bound))) => {
            wildcard_contains(db, scope, sub_bound, sup_bound)
        }
        // A wildcard source against a concrete parameter is never a subtype:
        // after capture conversion (§5.1.10) the captured type variable is
        // distinct from the concrete argument (§4.10.2 invariance), so
        // `List<? extends A> <: List<B>` does not hold.
        (TyKind::Wildcard(_), _) => false,
        // A concrete argument against a bounded wildcard parameter (§4.5.1):
        // `T <= ? extends S` iff `T <: S`; `T <= ? super S` iff `S <: T`.
        (_, TyKind::Wildcard(Some(bound))) => match bound.kind {
            BoundKind::Upper => is_subtype_query(db, scope, sub.id, bound.ty.id),
            BoundKind::Lower => is_subtype_query(db, scope, bound.ty.id, sub.id),
        },
        // Invariance (§4.10.2): `G<T> <: G<T'>` iff `T = T'`.
        _ => sub == sup,
    }
}

/// The wildcard "contains" relation
/// ([JLS §4.5.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.5.1)),
/// as sets of types: `? extends A` denotes `{ X : X <: A }` and `? super A`
/// denotes `{ X : A <: X }`, and the source argument must be contained in the
/// parameter argument. The only mixed-upper/lower rule is
/// `? super T <= ? extends Object`, so `(Upper, Lower)` never holds and
/// `(Lower, Upper)` holds only when the upper bound is `Object`.
fn wildcard_contains(
    db: &dyn TyDatabase,
    scope: ScopeId,
    sub: &WildcardBound,
    sup: &WildcardBound,
) -> bool {
    match (sub.kind, sup.kind) {
        // `? extends A <= ? extends B` iff `A <: B`.
        (BoundKind::Upper, BoundKind::Upper) => is_subtype_query(db, scope, sub.ty.id, sup.ty.id),
        // `? super A <= ? super B` iff `B <: A`.
        (BoundKind::Lower, BoundKind::Lower) => is_subtype_query(db, scope, sup.ty.id, sub.ty.id),
        // `? extends A <= ? super B` is never provable (§4.5.1).
        (BoundKind::Upper, BoundKind::Lower) => false,
        // `? super A <= ? extends B` iff `B` is `Object` (§4.5.1).
        (BoundKind::Lower, BoundKind::Upper) => sup.ty.is_object(db),
    }
}
