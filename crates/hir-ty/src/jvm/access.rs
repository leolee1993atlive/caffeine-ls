//! Access control for a JVM member
//! ([JLS §6.6](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.6))
//! and the invocation-mode filter of
//! [§15.12.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-15.html#jls-15.12.3).
//!
//! The rules take the member's already-computed [`Access`] and [`MethodData`];
//! a language's item tree is read only through [`crate::lang`].

use hir_expand::name::Name;

use crate::jvm::db::TyDatabase;
use crate::jvm::member::{Access, ClassKey, MethodData};
use crate::jvm::member_set::InvocationContext;
use crate::jvm::member_set::InvocationMode;
use crate::ty::Ty;

/// Whether `method` is allowed by the invocation mode of `ctx`
/// ([JLS §15.12.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-15.html#jls-15.12.1),
/// [§15.12.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-15.html#jls-15.12.3)).
pub(crate) fn mode_allows(method: &MethodData, ctx: &InvocationContext) -> bool {
    match ctx.mode {
        // A static invocation selects only static members.
        InvocationMode::Static => method.is_static,
        // §15.13.1: a type-qualified method reference may reference a static or
        // an unbound instance member, so every member of the name is a
        // candidate.
        InvocationMode::TypeQualified => true,
        // Super and interface invocations select only instance members.
        InvocationMode::Super | InvocationMode::Interface => !method.is_static,
        // A language that reaches no static member through a receiver
        // expression: only the instance members are candidates, own statics
        // included ([`InvocationMode::InstanceReceiver`]).
        InvocationMode::InstanceReceiver => !method.is_static,
        // §15.12.3 MethodName form: the member set is the virtual one plus the
        // static interface methods the searched type itself declares. The
        // static-interface-owner rule (declared-in-receiver-only, never
        // inherited) is applied by [`member_set_impl`], which knows the
        // receiver; no blanket static filter applies here.
        InvocationMode::MethodName => true,
        // A virtual invocation must not select a static method declared in an
        // interface (§15.12.3); a static method of a class may be selected.
        InvocationMode::Virtual => !(method.is_static && method.declaring_interface),
    }
}

/// Whether `method` is accessible to the class in which the invocation appears
/// ([JLS §6.6](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.6)),
/// when accessed through the receiver expression of type `receiver`.
pub(crate) fn is_accessible(
    db: &dyn TyDatabase,
    scope: &jvm_index::ResolutionScope,
    method: &MethodData,
    receiver: &Ty,
    ctx: &InvocationContext,
) -> bool {
    member_accessible(
        db,
        scope,
        method.access,
        method.declaring_package.as_deref(),
        &method.owner,
        method.declaring_top_level.as_deref(),
        receiver,
        method.is_static,
        ctx,
    )
}

/// Whether a member with `access` declared in `declaring_package` by the class
/// `owner` (whose top-level class is `declaring_top_level`) is accessible to
/// the class in which the access appears
/// ([JLS §6.6](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.6)),
/// when accessed through the receiver expression of type `receiver`. The two
/// names serve different rules: §6.6.1 scopes *private* access by the
/// top-level class, while §6.6.2 requires a subclass of the *declaring* class.
#[allow(clippy::too_many_arguments)]
pub(crate) fn member_accessible(
    db: &dyn TyDatabase,
    scope: &jvm_index::ResolutionScope,
    access: Access,
    declaring_package: Option<&str>,
    owner: &ClassKey,
    declaring_top_level: Option<&str>,
    receiver: &Ty,
    static_member: bool,
    ctx: &InvocationContext,
) -> bool {
    match access {
        Access::Public => true,
        // §6.6.1: a private member is accessible throughout the top-level
        // class in which it is declared — a local declaration's top level is
        // the class enclosing it.
        Access::Private => match (&ctx.enclosing_class, declaring_top_level) {
            (Some(enclosing), Some(declaring)) => {
                let enclosing = enclosing.top_level(db, ctx.package.as_deref());
                match enclosing {
                    Some(enclosing) => within_top_level(enclosing.as_str(), declaring),
                    None => false,
                }
            }
            _ => false,
        },
        // §6.6.1: a package member is accessible only within its own package;
        // the unnamed package ([§7.4.2]) is `""`.
        Access::Package => match (&ctx.package, declaring_package) {
            (Some(invocation), Some(declaring)) => invocation == declaring,
            _ => false,
        },
        // §6.6.2: a protected member is accessible within the declaring
        // package, or from a class that is a subclass of the declaring class.
        Access::Protected => {
            if let (Some(invocation), Some(declaring)) = (&ctx.package, declaring_package)
                && invocation == declaring
            {
                return true;
            }
            // §6.6.2: code that is "responsible for the implementation of an
            // object of the subclass" — an *anonymous class body* whose direct
            // superclass is the declaring class — may access the protected
            // members the superclass itself declares, from any package (the
            // Gson `new TypeToken<T>() {}` idiom: an anonymous subclass of the
            // generic `TypeToken` invokes its protected no-arg constructor).
            if let Some(subclass_of) = &ctx.subclass_of {
                let subclass_of = subclass_of.as_ty(db, Vec::new());
                let declaring = owner.as_ty(db, Vec::new());
                // The anonymous class subclasses the *created* type exactly;
                // members it inherits from a *grand*-supertype of that type are
                // reachable only when the created type itself is a subclass,
                // which this membership already entails via the transitive
                // subtype check below.
                if crate::jvm::subtyping::is_subtype(db, scope, &subclass_of, &declaring) {
                    return true;
                }
            }
            let declaring = owner.as_ty(db, Vec::new());
            let subclass = ctx.enclosing_class.as_ref().and_then(|enclosing| {
                // §6.6.2: the access may appear in the body of a *nested*
                // class of a subclass — `B.Inner2` inside `B extends A`
                // accessing A's protected members — where the *innermost*
                // enclosing class `app.B.Inner2` is not itself a subclass but
                // its enclosing `app.B` is. Walk the enclosing chain outward
                // and find the subclass `S` whose body contains the access.
                enclosing_class_keys(db, ctx.package.as_deref(), enclosing)
                    .into_iter()
                    .find(|candidate| {
                        let candidate = candidate.as_ty(db, Vec::new());
                        crate::jvm::subtyping::is_subtype(db, scope, &candidate, &declaring)
                    })
            });
            match subclass {
                // §6.6.2: a protected instance member accessed outside the
                // declaring package by a receiver expression requires the type
                // of that expression to be a subtype of the subclass `S` whose
                // body the access is in — a `B.this.value(...)` inside
                // `B.Inner2` accesses through a receiver of type `B` (= S). A
                // `super` invocation accesses the member through the `super`
                // keyword, not an expression, so the rule does not apply
                // ([§15.12.1]).
                Some(_subclass) if static_member || ctx.mode == InvocationMode::Super => true,
                Some(subclass) => {
                    let subclass = subclass.as_ty(db, Vec::new());
                    crate::jvm::subtyping::is_subtype(db, scope, receiver, &subclass)
                }
                None => false,
            }
        }
    }
}

/// Whether some declared type parameter of the class `fqn` has a bound that
/// references the class itself — the *SELF* channel of a fluent API
/// (`SELF extends AbstractStringAssert<SELF>`), as opposed to a plain element
/// parameter (`Enumeration<E>`).
/// The indexes of the declared type parameters of the class `fqn` whose
/// bounds reference the class itself — the *SELF* channel of a fluent API
/// (`SELF extends AbstractStringAssert<SELF>`), as opposed to a plain element
/// parameter (`Enumeration<E>`). Library-only resolution (source classes
/// return empty — their bounds are source references resolved per class, and
/// the fluent-SELF rewriting targets the assertj-style library stubs);
/// `Some`/`None` per resolved class, `None` when the class or its generic
/// info is unavailable.
pub(crate) fn self_type_param_indexes(
    db: &dyn TyDatabase,
    scope: &jvm_index::ResolutionScope,
    fqn: &str,
) -> Option<Vec<usize>> {
    let resolved = jvm_index::fqn_resolve(db, scope, fqn)?;
    let params = match resolved {
        jvm_index::Resolved::Library(_) => {
            jvm_index::class_generic_info(db, &resolved)?.type_params
        }
        jvm_index::Resolved::Source(_) | jvm_index::Resolved::Facade { .. } => {
            return Some(Vec::new());
        }
    };
    fn mention(
        bound: &classfile::stub::TypeRef<jvm_index::Symbol>,
        self_fqn: &str,
        interner: &dyn jvm_index::HirDatabase,
    ) -> bool {
        match bound {
            classfile::stub::TypeRef::Reference { name, generic_args } => {
                interner.hir_state().interner.resolve(name) == self_fqn
                    || generic_args
                        .iter()
                        .any(|arg| mention(arg, self_fqn, interner))
            }
            _ => false,
        }
    }
    Some(
        params
            .iter()
            .enumerate()
            .filter_map(|(i, param)| {
                param
                    .bounds
                    .iter()
                    .any(|bound| mention(bound, fqn, db))
                    .then_some(i)
            })
            .collect(),
    )
}

/// The class an access appears in and each class enclosing it, innermost first
/// ([JLS §6.6.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-6.html#jls-6.6.2)):
/// the §6.6.2 protected-access rule looks for the enclosing subclass whose
/// body contains the access, which may be an enclosing class rather than the
/// innermost one. A *local* declaration walks its real declaration chain; a
/// canonical name is trimmed segment by segment, bounded by the package.
fn enclosing_class_keys(
    db: &dyn TyDatabase,
    package: Option<&str>,
    key: &ClassKey,
) -> Vec<ClassKey> {
    let mut out = vec![key.clone()];
    match key {
        ClassKey::Local(class) => {
            // The declaration chain is the declaring language's item tree.
            // A language that does not record one contributes nothing, which
            // is what a classfile (and a non-Java local) already did.
            if let Some(language) = crate::lang::for_file(db, class.file) {
                out.extend(
                    language
                        .member_source()
                        .enclosing_named_classes(db, class)
                        .into_iter()
                        .map(ClassKey::Named),
                );
            }
        }
        ClassKey::Named(fqn) => {
            let package_prefix = match package {
                Some(package) if !package.is_empty() => format!("{package}."),
                _ => String::new(),
            };
            let mut candidate = fqn.as_str().to_owned();
            while let Some((prefix, _)) = candidate.rsplit_once('.') {
                if !prefix.starts_with(&package_prefix) {
                    break;
                }
                out.push(ClassKey::Named(Name::new(prefix)));
                candidate = prefix.to_owned();
            }
        }
    }
    out
}

/// Whether the class `enclosing` is the top-level class `declaring` or
/// lexically inside it ([JLS §6.6.1]): a private member is accessible
/// throughout the body of its top-level class, including from nested classes.
/// `declaring_top_level` is always the top level itself, so containment is
/// plain dot-prefix matching; the access site's enclosing class is a *source*
/// dotted name (inference only runs on source), so `$` never separates it.
fn within_top_level(enclosing: &str, declaring: &str) -> bool {
    enclosing == declaring || enclosing.starts_with(&format!("{}.", declaring))
}
