//! Subtyping and assignability.
//!
//! [`is_subtype`] implements the subtype relation of
//! [JLS §4.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10):
//! identity, the primitive partial order
//! ([§4.10.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10.1),
//! which is identity only), the class and interface transitive closure
//! ([§4.10.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10.2))
//! and array covariance
//! ([§4.10.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10.3)).
//! [`is_assignable`] implements assignment conversion
//! ([JLS §5.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.2))
//! via identity, primitive widening
//! ([§5.1.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.2))
//! and reference widening
//! ([§5.1.5](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.5)).
//!
//! The per-scope subtype and supertype results are memoized as tracked
//! queries keyed on the interned scope ([`crate::jvm::db::ScopeId`]) and the
//! interned type id ([`crate::java::ty::TyData`]), so repeated checks of the same
//! pair — the IDE pattern — hit the query cache. The public functions
//! intern the scope and the types, then delegate.
//!
//! Boxing/unboxing
//! ([§5.1.7](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.7),
//! [§5.1.8](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.8))
//! and capture conversion
//! ([§5.1.10](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.10),
//! via the §4.5.1 contains relation) are modelled for assignment conversion
//! and parameterized-type arguments.

use classfile::stub::{PrimitiveType, TypeRef};
use rustc_hash::FxHashSet;

use hir_def::java::item_tree::{ItemData, TypeParam};
use hir_def::jvm::access::JvmAccessFlags;
use hir_def::jvm::decl::ItemTypeRef;
use hir_expand::name::Name;

use crate::{
    java::resolve::{Resolver, item_data, resolve_type_ref, scope_for_file},
    java::ty::{Ty, TyKind, boxed_type, unboxed_primitive},
    jvm::db::{ScopeId, ScopeKind, TyDatabase},
    lang::JvmMemberSource,
};

/// Whether the source class-like declaration is an `interface` (or
/// `@interface`): its supertype set gains `java.lang.Object` explicitly
/// ([JLS §4.10.2], [§9.1]).
fn is_source_interface(db: &dyn TyDatabase, source: hir::SourceClass) -> bool {
    let tree = hir_def::java::plugin::tree(db, source.file);
    matches!(
        item_data(&tree, source.item),
        Some(ItemData::Interface(_)) | Some(ItemData::Annotation(_))
    )
}

/// The direct supertypes [`crate::lang::LanguageTypes::supertypes`] answers for
/// a Java declaration: the item-tree arm, with `java.lang.Object` added for an
/// interface. A library class is not answered here.
pub(crate) fn java_language_supertypes(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    ty: Ty,
) -> Vec<Ty> {
    let TyKind::Reference { args, .. } = ty.kind(db) else {
        return Vec::new();
    };
    let Some(resolved) = crate::jvm::resolve::reference_class(db, scope, &ty) else {
        return Vec::new();
    };
    match resolved {
        hir::Resolved::Source(source) => {
            let mut out = java_source_supertypes(db, source, args);
            if hir_def::java::plugin::declares_file(db, source.file)
                && is_source_interface(db, source)
            {
                let object = Ty::reference(db, "java.lang.Object", Vec::new());
                if !out.contains(&object) {
                    out.push(object);
                }
            }
            out
        }
        // A facade is not a Java declaration. The shared walk asks the file's
        // language; this arm only answers if a Java file is asked directly.
        hir::Resolved::Facade { .. } => vec![Ty::reference(db, "java.lang.Object", Vec::new())],
        hir::Resolved::Library(_) => Vec::new(),
    }
}

/// The direct supertypes of a source class, resolved against its own file's
/// scope ([JLS §4.10.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-4.html#jls-4.10.2)).
/// When the class declares type parameters and `args` are present, the
/// parameters are bound to the arguments and substituted into the
/// superclass/interfaces; otherwise they are resolved raw. Enums, records and
/// annotations gain their implicit superclass (`java.lang.Enum`,
/// `java.lang.Record`, `java.lang.annotation.Annotation`).
pub(crate) fn java_source_supertypes(
    db: &dyn TyDatabase,
    source: hir::SourceClass,
    args: &[Ty],
) -> Vec<Ty> {
    // A class of another language: its own type layer answers, and a Java
    // caller reads the supertypes in their JVM form
    // ([`LanguageTypes::ty_to_jvm`]) — a Kotlin class extending `JFrame` is a
    // Java subtype of `javax.swing.JFrame`.
    if !hir_def::java::plugin::declares_file(db, source.file) {
        let Some(language) = crate::lang::for_file(db, source.file) else {
            return Vec::new();
        };
        let ty = Ty::reference(
            db,
            hir::source_class_fqn(db, source.file, source.item).unwrap_or_else(|| Name::new("")),
            args.to_vec(),
        );
        let scope = scope_for_file(db, source.file);
        return language
            .supertypes(db, &scope, ty)
            .into_iter()
            .map(|supertype| language.ty_to_jvm(db, supertype))
            .collect();
    }
    let tree = hir_def::java::plugin::tree(db, source.file);
    let Some(data) = item_data(&tree, source.item) else {
        return Vec::new();
    };
    let scope = scope_for_file(db, source.file);
    let resolver = Resolver::for_item(db, source.file, &tree, source.item);
    let implicit = |fqn: &str| {
        ItemTypeRef::synthetic(TypeRef::Reference {
            name: Name::new(fqn),
            generic_args: Vec::new(),
        })
    };
    let (super_class, interfaces): (Option<ItemTypeRef>, Vec<ItemTypeRef>) = match data {
        // A class (other than `java.lang.Object`) has exactly one direct
        // superclass; the implicit one is `java.lang.Object` (§8.1.4).
        ItemData::Class(d) => (
            Some(
                d.super_class
                    .clone()
                    .unwrap_or_else(|| implicit("java.lang.Object")),
            ),
            d.interfaces.clone(),
        ),
        ItemData::Interface(d) => (None, d.interfaces.clone()),
        ItemData::Record(d) => (Some(implicit("java.lang.Record")), d.interfaces.clone()),
        // §8.9.2: the direct superclass of an enum type `E` is the
        // *parameterized* `java.lang.Enum<E>` — its own name as the type
        // argument — not the raw `java.lang.Enum`. A raw superclass erases
        // the `Enum<E>` bound ([§4.8]), so `E <: java.lang.Enum<E>` would
        // fail and a `EnumSet<E>`/`EnumMap<E,…>` use would report its bound
        // violated; with the parameterized superclass the enum is a subtype
        // of its own `Enum<E>` supertype as javac resolves it.
        ItemData::Enum(d) => (Some(enum_superclass(db, source)), d.interfaces.clone()),
        ItemData::Annotation(_) => (
            Some(implicit("java.lang.annotation.Annotation")),
            Vec::new(),
        ),
        _ => return Vec::new(),
    };
    let declared: &[TypeParam] = match data {
        ItemData::Class(d) | ItemData::Interface(d) => &d.type_params,
        ItemData::Record(d) => &d.type_params,
        _ => &[],
    };
    let instantiate = |tyref: &ItemTypeRef| {
        let resolved = resolve_type_ref(db, &scope, &resolver, tyref);
        if args.is_empty() {
            resolved
        } else {
            // §4.10.2 with [§4.4]/[§6.3]: bind the class's *own* declared
            // parameters, keyed by their declaring scopes.
            let binding = crate::java::resolve::source_class_binding(
                source.file,
                source.item,
                declared,
                args,
            );
            resolved.substitute(db, &binding)
        }
    };
    let mut out = Vec::new();
    if let Some(super_class) = &super_class {
        out.push(instantiate(super_class));
    }
    out.extend(interfaces.iter().map(instantiate));
    out
}

/// §8.9.2: the implicit direct superclass of the enum `source` —
/// `java.lang.Enum<E>`, where `E` is the enum's own type. The enum type is
/// never generic ([§8.9]), so the self type argument is the enum's fully
/// qualified name with no type arguments of its own.
fn enum_superclass(db: &dyn TyDatabase, source: hir::SourceClass) -> ItemTypeRef {
    let fqn = hir::source_class_fqn(db, source.file, source.item)
        .map(|name| name.as_str().to_owned())
        .unwrap_or_default();
    ItemTypeRef::synthetic(TypeRef::Reference {
        name: Name::new("java.lang.Enum"),
        generic_args: vec![TypeRef::Reference {
            name: Name::new(&fqn),
            generic_args: Vec::new(),
        }],
    })
}

/// Resolves `name` against the classes of `scope`, honoring classpath order:
/// a source set's own classes, then its classpath entries.
fn resolve_name(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    name: &Name,
) -> Option<hir::Resolved> {
    hir::fqn_resolve(db, scope, name.as_str())
}

/// The declared constant names of the named type, when it is an `enum`
/// ([JLS §8.9](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.9)):
/// a source enum lists its `EnumConstant` items, a library enum its
/// ACC_ENUM-flagged fields ([JVMS §4.6]). A non-enum or unresolvable type
/// yields `None`.
pub(crate) fn enum_constants(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    ty: &Ty,
) -> Option<Vec<Name>> {
    let (name, _) = ty.as_reference(db)?;
    match resolve_name(db, scope, name)? {
        // A Kotlin file's facade is no enum.
        hir::Resolved::Facade { .. } => None,
        hir::Resolved::Library(library) => {
            let record = hir::class_record(db, &library)?;
            let classfile::stub::ClassOrModuleStub::Class(class) = record.as_ref() else {
                return None;
            };
            // JVMS §4.6: enum constants carry the ACC_ENUM flag.
            if !JvmAccessFlags::from_bits_retain(class.flags).is_enum() {
                return None;
            }
            let interner = &db.hir_state().interner;
            Some(
                class
                    .fields
                    .iter()
                    .filter(|field| JvmAccessFlags::from_bits_retain(field.flags).is_enum())
                    .map(|field| Name::new(interner.resolve(&field.name)))
                    .collect(),
            )
        }
        hir::Resolved::Source(source) => {
            let tree = hir_def::java::plugin::tree(db, source.file);
            let ItemData::Enum(data) = item_data(&tree, source.item)? else {
                return None;
            };
            Some(
                data.body
                    .iter()
                    .filter_map(|&item| match item_data(&tree, item) {
                        Some(ItemData::EnumConstant(constant)) => Some(constant.name.clone()),
                        _ => None,
                    })
                    .collect(),
            )
        }
    }
}

/// Whether the named reference type is a *class-like* type (`class`, `enum`
/// or `record`, as opposed to an interface or annotation) and whether it is
/// `final` ([JLS §8.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.1),
/// [§8.1.1.2], [§8.9], [§9.1]). Unresolvable names yield `None`; callers must
/// stay permissive there.
///
/// The answer is the *Java* layer's own — a Java declaration's items and a
/// classfile's flags — because that is what this function's callers
/// ([`super::decl_check`]) ask about. A site that must classify a class of any
/// language asks the registry instead ([`crate::jvm::member_set::class_kind`]).
pub(crate) fn class_like_and_final(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    ty: &Ty,
) -> Option<(bool, bool)> {
    let (name, _) = ty.as_reference(db)?;
    let resolved = resolve_name(db, scope, name)?;
    let (kind, final_) = match &resolved {
        hir::Resolved::Library(_) => crate::lang::classfile()
            .member_source()
            .kind(db, &resolved)?,
        other => crate::java::plugin::JAVA.kind(db, other)?,
    };
    Some((!kind.is_interface(), final_))
}

/// Whether the named reference type is an interface or annotation type — the
/// supertype kind [JLS §8.1.5]/[§9.1.3] require of every type named by an
/// `implements` clause or by an interface's `extends` clause. `None` when the
/// name does not resolve to a reference type (or resolves to nothing), so
/// callers stay permissive there.
pub(crate) fn is_interface_type(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    ty: &Ty,
) -> Option<bool> {
    class_like_and_final(db, scope, ty).map(|(class_like, _)| !class_like)
}

/// Whether `src` is assignable to `dst` by assignment conversion
/// ([JLS §5.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.2)):
/// identity, primitive widening
/// ([§5.1.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.2)),
/// reference widening
/// ([§5.1.5](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.5)),
/// boxing ([§5.1.7](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.7),
/// optionally followed by a widening reference conversion) and unboxing
/// ([§5.1.8](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.8),
/// optionally followed by a widening primitive conversion).
pub fn is_assignable(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    src: &Ty,
    dst: &Ty,
) -> bool {
    if src == dst {
        return true;
    }
    // §4.10.2/[§5.2]: a parameterized type whose type variables are the same
    // *by name* is identical even when the two handles intern differently —
    // the recursion guard of §4.4 bound resolution truncates a
    // self-referential bound (`T extends Box<K,T>`) at different depths per
    // resolver context. Subtyping cannot decide the pair (`is_subtype` has no
    // reflexivity for separately-interned same-shape handles), so the
    // assignment/return of an identical pair ([§5.2], [§14.17]) is accepted
    // directly here.
    if src.same_shape(db, dst) {
        return true;
    }
    match (src.kind(db), dst.kind(db)) {
        (TyKind::Primitive(src), TyKind::Primitive(dst)) => widening_primitive(*src, *dst),
        (
            TyKind::Reference { .. } | TyKind::Intersection(_),
            TyKind::Reference { .. } | TyKind::Intersection(_),
        ) => {
            crate::jvm::subtyping::is_subtype(db, scope, src, dst)
                // §5.1.9/§5.2: a raw type converts to any parameterization of
                // its own generic class by *unchecked conversion* — legal in
                // assignment context, reported as a warning by the type layer.
                || unchecked_conversion(db, scope, src, dst)
        }
        // Boxing (§5.1.7): a primitive is assignable to its boxed type, or to
        // any reference supertype of it (a widening reference conversion §5.1.5
        // after boxing).
        (TyKind::Primitive(src), TyKind::Reference { .. }) => {
            let boxed = Ty::reference(db, boxed_type(*src), Vec::new());
            crate::jvm::subtyping::is_subtype(db, scope, &boxed, dst)
        }
        // Unboxing (§5.1.8): a boxed reference is assignable to the primitive
        // it unboxes to, or to a wider primitive (a widening primitive
        // conversion §5.1.2 after unboxing).
        (TyKind::Reference { name, .. }, TyKind::Primitive(dst)) => {
            let Some(unboxed) = unboxed_primitive(name.as_str()) else {
                return false;
            };
            unboxed == *dst || widening_primitive(unboxed, *dst)
        }
        // §4.10.3/§5.2: an array converts to its class supertypes —
        // `Object`, `Cloneable`, `Serializable` — and through them to any
        // reference type by subtyping.
        (TyKind::Array(_), TyKind::Reference { .. } | TyKind::TypeVar { .. }) => {
            crate::jvm::subtyping::is_subtype(db, scope, src, dst)
        }
        // §4.10.3/§5.2: array to array — element-wise covariance (a widening
        // reference conversion of the elements, so `Frame<BasicValue>[]` is
        // assignable to `Frame<?>[]`), plus the §5.1.9 *unchecked* conversion
        // of an array of a raw type: `Frame[]` (from `new Frame[7]`) converts
        // to `Frame<BasicValue>[]` exactly as the raw element type does.
        (TyKind::Array(si), TyKind::Array(ti)) => {
            if crate::jvm::subtyping::is_subtype(db, scope, src, dst) {
                return true;
            }
            !si.contains_infer_var(db)
                && !ti.contains_infer_var(db)
                && unchecked_conversion(db, scope, si, ti)
        }
        // §5.1.8 with §5.1.10: an expression typed by a *captured* type
        // variable with a lower bound `L` (the capture of `? super L`) holds
        // an `L`, which unboxes to its primitive.
        (
            TyKind::TypeVar {
                lower: Some(lower), ..
            },
            TyKind::Primitive(dst),
        ) => match lower.kind(db) {
            TyKind::Reference { name, .. } => {
                let Some(unboxed) = unboxed_primitive(name.as_str()) else {
                    return false;
                };
                unboxed == *dst || widening_primitive(unboxed, *dst)
            }
            _ => false,
        },
        // §5.2/§4.10.2: a type-variable-typed expression converts by the
        // reference conversions of its declared bounds, and a target that is
        // itself a type variable admits sources below its lower bound (the
        // §5.1.10 capture of `? super L`). Both are exactly the subtyping
        // judgments, so delegate; a primitive source boxes first (§5.1.7).
        (TyKind::Primitive(src), TyKind::TypeVar { .. }) => {
            let boxed = Ty::reference(db, boxed_type(*src), Vec::new());
            crate::jvm::subtyping::is_subtype(db, scope, &boxed, dst)
        }
        (TyKind::TypeVar { .. }, _)
        | (
            TyKind::Reference { .. } | TyKind::Intersection(_) | TyKind::Null,
            TyKind::TypeVar { .. },
        ) => {
            crate::jvm::subtyping::is_subtype(db, scope, src, dst)
                // §5.1.9 with §4.10.2: a type variable whose *bound* is a raw
                // type converts to a parameterization of that class by
                // unchecked conversion — `CAP <: Cond` (raw) from a
                // `Function<JsonObject, ? extends Cond>` receiver, assigned to
                // a `Cond<?>` target, is exactly the raw→parameterized
                // unchecked conversion javac admits.
                || (!dst.is_type_var(db)
                    && src
                        .bounds(db)
                        .iter()
                        .any(|bound| unchecked_conversion(db, scope, bound, dst)))
        }
        // §5.1.4/§5.1.5: the null literal is assignable to any reference type.
        (TyKind::Null, _) => crate::jvm::subtyping::is_subtype(db, scope, src, dst),
        _ => false,
    }
}

/// §5.1.9: whether `src` is a *raw type* — a parameterized class used without
/// its type arguments — whose erasure has `dst`'s erasure among its
/// supertypes. The conversion is unchecked ([§5.2]): legal, but carrying no
/// element-type guarantee. An *array* of such a raw element converts
/// unchecked to an array of the parameterized element ([§5.1.9], [§10]): the
/// runtime array type is the erased element's array, so the conversion is
/// exactly the element-level unchecked conversion with the array dimensions
/// peeled on both sides.
fn unchecked_conversion(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    src: &Ty,
    dst: &Ty,
) -> bool {
    // §5.1.9/[§10]: `raw E[]` converts unchecked to `param E'[]` iff `raw E`
    // converts unchecked to `param E'` — peel the array dimensions on both
    // sides and recurse on the elements.
    match (src.kind(db), dst.kind(db)) {
        (TyKind::Array(si), TyKind::Array(ti)) => {
            return unchecked_conversion(db, scope, si, ti);
        }
        (TyKind::Array(_), _) | (_, TyKind::Array(_)) => return false,
        _ => {}
    }
    let Some((src_name, src_args)) = src.as_reference(db) else {
        return false;
    };
    let Some((dst_name, dst_args)) = dst.as_reference(db) else {
        return false;
    };
    let raw_src = src_args.is_empty();
    if !dst_args.is_empty() && src_name == dst_name {
        // §5.1.9: the destination may be a parameterization of the source's
        // own generic class — `raw List` converts to `List<String>` directly.
        // (A *parameterized* source of the same class also passes here —
        // `G<A> → G<B>` — including capture-vs-concrete and unbound-var
        // pairs; javac decides those contexts individually, and the checked
        // subtyping has already failed, so the callers' lenient paths are
        // what keep the established `G<Object>` vs `G<Entry>` method-ref
        // probes legal.)
        return true;
    }
    // §5.1.9: the walk reaches the destination's erasure through the
    // supertype chain. For a raw source every supertype is erased, so the
    // first occurrence converts; a parameterized source (whose declared
    // supertype is raw — `DirectAccessor<K,T> extends ValueAccessor` converts
    // to `ValueAccessor<String>` with a warning) must find a *raw*
    // occurrence, and a parameterized occurrence with the target erasure
    // (invariance §4.10.2 makes the arguments incompatible — `ArrayList<Number>`
    // against `List<String>` reaches the parameterized `List<Number>`, exactly
    // the `List<String> l = new ArrayList<Number>()` error) is not an
    // unchecked conversion.
    let scope = ScopeId::new(db, ScopeKind::from_scope(scope));
    let mut stack = vec![*src];
    let mut visited = FxHashSet::default();
    while let Some(current) = stack.pop() {
        for parent in crate::jvm::subtyping::supertypes_query(db, scope, current.id) {
            if !visited.insert(parent.id) {
                continue;
            }
            match parent.as_reference(db) {
                // The erased supertype matches the destination's erasure.
                Some((parent_name, parent_args)) if parent_name == dst_name => {
                    if raw_src || parent_args.is_empty() {
                        return true;
                    }
                    // A same-type write (`ArrayList<X>` to `List<X>` with the
                    // source class's own type variables — the
                    // `ArrayList<ValueChangeValidator<T,K>>` to
                    // `List<ValueChangeValidator<T,K>>` field initializer) is
                    // a *checked* subtype that the memoized supertype walk
                    // loses across interned handles; its arguments are
                    // structurally identical, so accept exactly that pair
                    // (`same_shape`). A parameterized occurrence with
                    // different arguments — invariance §4.10.2, the
                    // `ArrayList<Number>` to `List<String>` error — is not an
                    // unchecked conversion.
                    if parent.same_shape(db, dst) {
                        return true;
                    }
                    return false;
                }
                Some(_) => stack.push(parent),
                None => {}
            }
        }
    }
    false
}

/// Whether `arg` is convertible to `param` by a strict invocation conversion
/// ([JLS §5.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.3)):
/// identity, widening primitive
/// ([§5.1.2](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.2)),
/// widening reference
/// ([§5.1.5](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.5)),
/// or unchecked
/// ([§5.1.9](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.9)).
/// Unlike assignment conversion there is no boxing or unboxing ([§5.3]);
/// boxing/unboxing are only admitted in the loose phase ([§5.2]).
pub(crate) fn strict_conversion(
    db: &dyn TyDatabase,
    scope: &hir::ResolutionScope,
    arg: &Ty,
    param: &Ty,
) -> bool {
    if arg == param {
        return true;
    }
    match (arg.kind(db), param.kind(db)) {
        (TyKind::Primitive(src), TyKind::Primitive(dst)) => widening_primitive(*src, *dst),
        _ => {
            crate::jvm::subtyping::is_subtype(db, scope, arg, param)
                // §5.1.9 with §5.3: the unchecked conversion (*raw* or
                // raw-array argument to a parameterized formal, or an array
                // of a raw element) is a strict invocation conversion too —
                // javac's phase-1 applicability test (`isSubtypeUnchecked`)
                // resolves `m(rawList)` to `m(List<String>)` in phase 1.
                || unchecked_conversion(db, scope, arg, param)
                // A type variable whose *bound* is a raw type converts to a
                // parameterization of that class by unchecked conversion
                // ([§5.1.9] with §4.10.2), mirroring `is_assignable`.
                || (!param.is_type_var(db)
                    && arg
                        .bounds(db)
                        .iter()
                        .any(|bound| unchecked_conversion(db, scope, bound, param)))
        }
    }
}

/// Primitive widening conversion, the transitions of
/// [JLS table 5.1-B](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.2).
pub(crate) fn widening_primitive(src: PrimitiveType, dst: PrimitiveType) -> bool {
    use PrimitiveType::*;
    matches!(
        (src, dst),
        (Byte, Short | Int | Long | Float | Double)
            | (Short, Int | Long | Float | Double)
            | (Char, Int | Long | Float | Double)
            | (Int, Long | Float | Double)
            | (Long, Float | Double)
            | (Float, Double)
    )
}

/// Whether a constant value is representable in the target primitive
/// ([JLS §5.1.3](https://docs.oracle.com/javase/specs/jls/se26/html/jls-5.html#jls-5.1.3)):
/// the narrowing-of-constants half of the assignment-context conversion
/// ([§5.2]) for `int` constants to `byte`, `short` and `char`.
pub(crate) fn fits_primitive(value: i64, dst: PrimitiveType) -> bool {
    use PrimitiveType::*;
    match dst {
        Byte => (i8::MIN as i64..=i8::MAX as i64).contains(&value),
        Short => (i16::MIN as i64..=i16::MAX as i64).contains(&value),
        Char => (0..=u16::MAX as i64).contains(&value),
        _ => false,
    }
}
