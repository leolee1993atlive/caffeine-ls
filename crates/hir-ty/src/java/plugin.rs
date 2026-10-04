//! Java as the type layer's registry entry: the JVM view of a Java class
//! ([`JvmMemberSource`]) and the Java type layer's own answers
//! ([`LanguageTypes`]).

use triomphe::Arc;

use base_db::LanguageKind;
use hir_def::java::item_tree::ItemData;
use hir_expand::name::Name;
use rustc_hash::FxHashSet;
use vfs::FileId;

use crate::{
    java::{
        method::{access_context, source_class_fields, source_class_methods},
        resolve::item_data,
        subtyping,
        ty::Ty,
    },
    jvm::db::TyDatabase,
    jvm::member::{FieldData, JvmClassKind, MethodData},
    jvm::member_set::InvocationContext,
    lang::{JvmMemberSource, LanguageTypes},
};

pub(crate) struct Java;

pub(crate) static JAVA: Java = Java;

impl LanguageTypes for Java {
    fn kinds(&self) -> &'static [LanguageKind] {
        &[LanguageKind::Java]
    }

    fn member_source(&self) -> &'static dyn JvmMemberSource {
        &JAVA
    }

    fn ty_from_jvm(&self, _db: &dyn TyDatabase, ty: Ty) -> Ty {
        // A JVM type *is* a Java type.
        ty
    }

    fn ty_to_jvm(&self, _db: &dyn TyDatabase, ty: Ty) -> Ty {
        ty
    }

    fn supertypes(
        &self,
        db: &dyn TyDatabase,
        scope: &jvm_index::ResolutionScope,
        ty: Ty,
    ) -> Vec<Ty> {
        subtyping::java_language_supertypes(db, scope, ty)
    }

    fn access_context(
        &self,
        db: &dyn TyDatabase,
        file: FileId,
        item: hir_expand::ids::ItemId,
    ) -> InvocationContext {
        access_context(db, file, item)
    }

    fn file_resolved_deps(&self, db: &dyn TyDatabase, file: FileId) -> Arc<FxHashSet<FileId>> {
        crate::java::db::file_resolved_deps(db, file)
    }

    fn file_dependency_refs(&self, db: &dyn TyDatabase, file: FileId) -> Arc<FxHashSet<Name>> {
        crate::java::db::file_dependency_refs(db, file)
    }

    fn deprecation(
        &self,
        db: &dyn TyDatabase,
        file: FileId,
        item: hir_expand::ids::ItemId,
    ) -> Option<crate::jvm::deprecation::Deprecation> {
        crate::java::deprecation::source_item(db, file, item)
    }

    fn declared_type_param_count(&self, db: &dyn TyDatabase, class: &jvm_index::Resolved) -> usize {
        let jvm_index::Resolved::Source(source) = class else {
            return 0;
        };
        let tree = hir_def::java::plugin::tree(db, source.file);
        let Some(data) = item_data(&tree, source.item) else {
            return 0;
        };
        match data {
            ItemData::Class(d) | ItemData::Interface(d) => d.type_params.len(),
            ItemData::Record(d) => d.type_params.len(),
            _ => 0,
        }
    }

    fn source_type_vars(&self, db: &dyn TyDatabase, class: &jvm_index::Resolved) -> Vec<Ty> {
        let jvm_index::Resolved::Source(class) = class else {
            return Vec::new();
        };
        let tree = hir_def::java::plugin::tree(db, class.file);
        let Some(data) = item_data(&tree, class.item) else {
            return Vec::new();
        };
        let declared = match data {
            ItemData::Class(d) | ItemData::Interface(d) => &d.type_params,
            ItemData::Record(d) => &d.type_params,
            _ => return Vec::new(),
        };
        let scope = crate::java::resolve::scope_for_file(db, class.file);
        let resolver = crate::java::resolve::Resolver::for_item(db, class.file, &tree, class.item);
        declared
            .iter()
            .map(|param| {
                let bounds = param
                    .bounds
                    .iter()
                    .map(|bound| {
                        crate::java::resolve::resolve_type_ref(db, &scope, &resolver, bound)
                    })
                    .collect();
                Ty::type_var(
                    db,
                    crate::ty::TypeVarScope::Class {
                        file: class.file,
                        item: class.item,
                        name: param.name.clone(),
                    },
                    bounds,
                )
            })
            .collect()
    }

    fn type_param_bounds(
        &self,
        db: &dyn TyDatabase,
        class: &jvm_index::Resolved,
    ) -> Vec<(crate::ty::TypeVarScope, Vec<Ty>)> {
        let jvm_index::Resolved::Source(source) = class else {
            return Vec::new();
        };
        crate::java::resolve::java_source_type_param_bounds(db, *source)
    }
}

impl JvmMemberSource for Java {
    fn methods(
        &self,
        db: &dyn TyDatabase,
        class: &jvm_index::Resolved,
        args: &[Ty],
        name: &str,
    ) -> Vec<MethodData> {
        let jvm_index::Resolved::Source(source) = class else {
            return Vec::new();
        };
        source_class_methods(db, *source, args.to_vec(), name)
    }

    fn fields(
        &self,
        db: &dyn TyDatabase,
        class: &jvm_index::Resolved,
        args: &[Ty],
        name: &str,
    ) -> Vec<FieldData> {
        let jvm_index::Resolved::Source(source) = class else {
            return Vec::new();
        };
        source_class_fields(db, *source, args.to_vec(), name)
    }

    fn abstract_methods(
        &self,
        db: &dyn TyDatabase,
        class: &jvm_index::Resolved,
        args: &[Ty],
    ) -> Vec<MethodData> {
        // §9.4.1: the abstract members of a Java interface are the methods its
        // declaration writes as abstract — an interface's other members are
        // `default` or `static` and are not part of the functional-interface
        // descriptor. A *class* declares no abstract member of its own here:
        // an abstract method it inherits is found by the caller's walk.
        let jvm_index::Resolved::Source(source) = class else {
            return Vec::new();
        };
        let tree = hir_def::java::plugin::tree(db, source.file);
        let Some(ItemData::Interface(class)) = item_data(&tree, source.item) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &item in &class.body {
            let Some(ItemData::Method(method)) = item_data(&tree, item) else {
                continue;
            };
            out.extend(source_class_methods(
                db,
                *source,
                args.to_vec(),
                method.name.as_str(),
            ));
        }
        out
    }

    /// The JVM kind of a Java class, from its classfile flags or its source
    /// modifiers ([JLS §8.1](https://docs.oracle.com/javase/specs/jls/se26/html/jls-8.html#jls-8.1),
    /// [§8.1.1.2], [§8.9], [§9.1]).
    fn kind(
        &self,
        db: &dyn TyDatabase,
        class: &jvm_index::Resolved,
    ) -> Option<(JvmClassKind, bool)> {
        match class {
            // A file's facade is a final class ([`crate::kotlin`]'s top-level
            // declarations compile into one).
            jvm_index::Resolved::Facade { .. } => Some((JvmClassKind::Facade, true)),
            // A library class is answered by the classfile entry, not here.
            jvm_index::Resolved::Library(_) => None,
            jvm_index::Resolved::Source(source) => {
                let tree = hir_def::java::plugin::tree(db, source.file);
                match item_data(&tree, source.item)? {
                    ItemData::Class(d) => Some((JvmClassKind::Class, d.modifiers.is_final())),
                    ItemData::Record(_) => Some((JvmClassKind::Class, true)),
                    // §8.9: an enum without constant bodies is implicitly final,
                    // but treating every enum as final only ever tightens a cast
                    // check that single inheritance already makes disjoint.
                    ItemData::Enum(_) => Some((JvmClassKind::Enum, true)),
                    ItemData::Interface(_) => Some((JvmClassKind::Interface, false)),
                    // §9.6: an annotation type *is* an interface, which is why
                    // it is a functional interface's candidate too.
                    ItemData::Annotation(_) => Some((JvmClassKind::Annotation, false)),
                    // A resolved *type reference* names one of the five
                    // class-like kinds above; anything else here is not a class
                    // at all, and the interface-like answer is the permissive
                    // one this classifier has always given.
                    _ => Some((JvmClassKind::Interface, false)),
                }
            }
        }
    }

    fn is_source_constructor(&self, db: &dyn TyDatabase, method: &MethodData) -> bool {
        let (Some(file), Some(item)) = (method.owner_file, method.decl_item) else {
            return false;
        };
        if !hir_def::java::plugin::declares_file(db, file) {
            return false;
        }
        let tree = hir_def::java::plugin::tree(db, file);
        matches!(
            item_data(&tree, item),
            Some(ItemData::Method(constructor)) if constructor.sig.ret.is_none()
        )
    }

    fn enclosing_named_classes(
        &self,
        db: &dyn TyDatabase,
        class: &jvm_index::SourceClass,
    ) -> Vec<Name> {
        let tree = hir_def::java::plugin::tree(db, class.file);
        crate::java::resolve::enclosing_type_chain(&tree, class.item)
    }
}
