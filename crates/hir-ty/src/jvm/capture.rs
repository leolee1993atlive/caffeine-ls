//! Capture conversion
//! ([JLS §5.1.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.10)).
//!
//! Applied to a wildcard-parameterized receiver before the shared member-set
//! walk. The fresh variables never reach the memoized subtype queries.

use hir_expand::name::Name;
use rustc_hash::FxHashMap;

use crate::jvm::db::TyDatabase;
use crate::ty::{BoundKind, Ty, TyKind, TypeVarScope};

/// Capture conversion ([JLS §5.1.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.10)):
/// replaces the wildcard type arguments of `ty` with fresh type variables,
/// so the member set of a wildcard-parameterized receiver sees a concrete
/// instantiation. `? extends T` becomes the fresh variable `CAP#<n>` bounded
/// by `T`; an unbounded `?` becomes `CAP#<n>` bounded by `Object`; `? super T`
/// becomes `CAP#<n>` bounded above by `Object` and below by `T`. Applied to
/// the receiver before the member set walk of
/// [`crate::jvm::member_set::member_set`], and only there: the capture variables never
/// reach the memoized subtype queries.
pub fn capture_conversion(db: &dyn TyDatabase, scope: &hir::ResolutionScope, ty: Ty) -> Ty {
    let fresh = |bound: Ty| Ty::fresh_capture(db, bound);
    match ty.kind(db) {
        TyKind::Reference { name, args, .. } => {
            // §5.1.10: the fresh variable of a *bare* `?` argument takes the
            // upper bound of the type parameter it fills — `AbstractLongAssert<?>`
            // captures to `AbstractLongAssert<CAP extends AbstractLongAssert<…>>`,
            // not to `AbstractLongAssert<CAP extends Object>`. Without it a
            // method returning the SELF type parameter (`as`, `contains`,
            // `isInstanceOf`) yields a bare wildcard, and the next chained
            // call degrades to "cannot find symbol" against the `Object`
            // bound.
            let placeholders: Vec<Option<Ty>> = args
                .iter()
                .map(|arg| {
                    if matches!(arg.kind(db), TyKind::Wildcard(_)) {
                        let id = crate::ty::next_capture();
                        Some(Ty::type_var(
                            db,
                            TypeVarScope::Capture {
                                id,
                                name: Name::new(&format!("CAP#{id}")),
                            },
                            Vec::new(),
                        ))
                    } else {
                        None
                    }
                })
                .collect();
            let declared = type_param_upper_bounds(db, scope, name.as_str(), args, &placeholders);
            ty.with_args(
                db,
                args.iter()
                    .enumerate()
                    .map(|(i, arg)| match arg.kind(db) {
                        TyKind::Wildcard(Some(bound)) => match bound.kind {
                            BoundKind::Upper => fresh(bound.ty),
                            // `? super T`: a capture variable with the `Object`
                            // upper bound and the `T` lower bound (§5.1.10).
                            BoundKind::Lower => Ty::captured_var(db, bound.ty),
                        },
                        TyKind::Wildcard(None) => {
                            let bound = declared.get(i).copied().unwrap_or_else(|| {
                                Ty::reference(db, "java.lang.Object", Vec::new())
                            });
                            fresh(bound)
                        }
                        // §5.1.10: capture conversion is *not* recursively
                        // applied to the non-wildcard type arguments — only the
                        // top-level wildcard arguments of the receiver's own
                        // type are replaced. A nested `Class<?>` inside
                        // `Map<Class<?>, String>` stays `Class<?>` (javac
                        // accepts `map.put(Class<Boolean>, …)` against it), and
                        // recursively capturing would turn it into a fresh
                        // `Class<CAP#n>` that no concrete `Class<B>` is a
                        // subtype of.
                        _ => *arg,
                    })
                    .collect(),
            )
        }
        TyKind::Array(inner) => Ty::array(db, **inner),
        TyKind::Wildcard(Some(bound)) => match bound.kind {
            BoundKind::Upper => fresh(bound.ty),
            // `? super T`: a capture variable with the `Object` upper bound
            // and the `T` lower bound (§5.1.10).
            BoundKind::Lower => Ty::captured_var(db, bound.ty),
        },
        TyKind::Wildcard(None) => fresh(Ty::reference(db, "java.lang.Object", Vec::new())),
        // Intersection and type-variable receivers are not parameterized by
        // wildcards; leave them as-is.
        _ => ty,
    }
}

/// The upper bound the type parameters of the resolved class declare, in
/// declaration order — what a *bare* `?` in each argument position captures
/// ([JLS §5.1.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.10)).
/// The declared bounds may reference the class's own type parameters
/// (`SELF extends AbstractAssert<SELF, …>`); those are substituted by the
/// argument in the corresponding position — a concrete argument by itself, a
/// wildcard argument by a fresh placeholder variable. A parameter without a
/// declared bound yields `Object`. Source classes and unresolvable names
/// (whose bounds need the source item tree) fall back to `Object`.
fn type_param_upper_bounds(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    fqn: &str,
    args: &[Ty],
    placeholders: &[Option<Ty>],
) -> Vec<Ty> {
    let object = Ty::reference(db, "java.lang.Object", Vec::new());
    // Scope-keyed so the *class's own* parameters are substituted ([§4.4],
    // [§6.3]) and a same-named variable of another declaration inside a bound
    // ([§6.4.1]) is left alone.
    let params = crate::jvm::resolve::declared_type_param_scopes(db, scope, &Name::new(fqn));
    if params.is_empty() && hir::fqn_resolve(db, scope, fqn).is_some() {
        // A class with no recoverable type parameters (unresolvable name or a
        // source declaration whose tree the helper cannot see) has none.
        return Vec::new();
    }
    let mut binding: FxHashMap<TypeVarScope, Ty> = FxHashMap::default();
    for (i, (var_scope, _)) in params.iter().enumerate() {
        let arg = match (args.get(i), placeholders.get(i)) {
            (_, Some(Some(ph))) => *ph,
            (Some(arg), _) => *arg,
            _ => object,
        };
        binding.insert(var_scope.clone(), arg);
    }
    params
        .iter()
        .map(|(_, bounds)| {
            bounds
                .first()
                .copied()
                .map(|bound| bound.substitute(db, &binding))
                .filter(|b| !b.is_object(db))
                .unwrap_or(object)
        })
        .collect()
}
