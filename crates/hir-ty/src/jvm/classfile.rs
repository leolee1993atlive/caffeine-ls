//! The classfile entry of the type-layer registry.
//!
//! A library class declares no source language. Callers that used to ask the
//! Java plugin for a classfile ask [`CLASSFILE`] instead, so the Java item
//! tree is never read for one.

use triomphe::Arc;

use hir_def::jvm::access::JvmAccessFlags;
use hir_expand::name::Name;
use rustc_hash::FxHashSet;
use vfs::FileId;

use crate::jvm::db::TyDatabase;
use crate::jvm::member::{ClassKey, FieldData, JvmClassKind, MethodData};
use crate::jvm::member_set::{InvocationContext, InvocationMode};
use crate::lang::{JvmMemberSource, LanguageTypes};
use crate::ty::{Ty, TyKind, TypeVarScope};

pub struct Classfile;

pub static CLASSFILE: Classfile = Classfile;

impl LanguageTypes for Classfile {
    fn kinds(&self) -> &'static [base_db::LanguageKind] {
        &[]
    }

    fn member_source(&self) -> &'static dyn JvmMemberSource {
        &CLASSFILE
    }

    fn ty_from_jvm(&self, _db: &dyn TyDatabase, ty: Ty) -> Ty {
        ty
    }

    fn ty_to_jvm(&self, _db: &dyn TyDatabase, ty: Ty) -> Ty {
        ty
    }

    fn supertypes(&self, db: &dyn TyDatabase, scope: &hir::ResolutionScope, ty: Ty) -> Vec<Ty> {
        library_supertypes(db, scope, ty)
    }

    fn access_context(
        &self,
        _db: &dyn TyDatabase,
        _file: FileId,
        _item: hir_expand::ids::ItemId,
    ) -> InvocationContext {
        // A classfile has no source call site. The probe context admits only
        // `public` members, which is what a library-only caller already used.
        InvocationContext {
            mode: InvocationMode::Virtual,
            enclosing_class: Some(ClassKey::Named(Name::new("library.probe.Caller"))),
            package: Some(String::new()),
            subclass_of: None,
        }
    }

    fn file_resolved_deps(&self, _db: &dyn TyDatabase, _file: FileId) -> Arc<FxHashSet<FileId>> {
        Arc::new(FxHashSet::default())
    }

    fn file_dependency_refs(&self, _db: &dyn TyDatabase, _file: FileId) -> Arc<FxHashSet<Name>> {
        Arc::new(FxHashSet::default())
    }

    fn deprecation(
        &self,
        _db: &dyn TyDatabase,
        _file: FileId,
        _item: hir_expand::ids::ItemId,
    ) -> Option<crate::jvm::deprecation::Deprecation> {
        None
    }

    fn declared_type_param_count(&self, db: &dyn TyDatabase, class: &hir::Resolved) -> usize {
        let hir::Resolved::Library(library) = class else {
            return 0;
        };
        hir::class_generic_info(db, &hir::Resolved::Library(library.clone()))
            .map(|info| info.type_params.len())
            .unwrap_or(0)
    }

    fn type_param_bounds(
        &self,
        db: &dyn TyDatabase,
        class: &hir::Resolved,
    ) -> Vec<(TypeVarScope, Vec<Ty>)> {
        crate::jvm::resolve::library_type_param_bounds(db, class)
    }
}

impl JvmMemberSource for Classfile {
    fn methods(
        &self,
        db: &dyn TyDatabase,
        class: &hir::Resolved,
        args: &[Ty],
        name: &str,
    ) -> Vec<MethodData> {
        let hir::Resolved::Library(library) = class else {
            return Vec::new();
        };
        crate::jvm::member_set::library_class_methods(db, library.clone(), args.to_vec(), name)
    }

    fn fields(
        &self,
        db: &dyn TyDatabase,
        class: &hir::Resolved,
        args: &[Ty],
        name: &str,
    ) -> Vec<FieldData> {
        let hir::Resolved::Library(library) = class else {
            return Vec::new();
        };
        crate::jvm::member_set::library_class_fields(db, library.clone(), args.to_vec(), name)
    }

    fn abstract_methods(
        &self,
        db: &dyn TyDatabase,
        class: &hir::Resolved,
        args: &[Ty],
    ) -> Vec<MethodData> {
        self.methods(db, class, args, "")
            .into_iter()
            .filter(|method| method.abstract_ && !method.is_static)
            .collect()
    }

    fn kind(&self, db: &dyn TyDatabase, class: &hir::Resolved) -> Option<(JvmClassKind, bool)> {
        let hir::Resolved::Library(library) = class else {
            return None;
        };
        let record = hir::class_record(db, library)?;
        let classfile::stub::ClassOrModuleStub::Class(class) = record.as_ref() else {
            return None;
        };
        // JVM access flags ([JVMS §4.1]): an interface carries ACC_INTERFACE
        // (an annotation type both it and ACC_ANNOTATION), an enum ACC_ENUM, a
        // final class ACC_FINAL. A record is implicitly final ([§8.10]).
        let flags = JvmAccessFlags::from_bits_retain(class.flags);
        let kind = if flags.is_interface() {
            match flags.is_annotation() {
                true => JvmClassKind::Annotation,
                false => JvmClassKind::Interface,
            }
        } else if flags.is_enum() {
            JvmClassKind::Enum
        } else {
            JvmClassKind::Class
        };
        Some((kind, flags.is_final() || class.is_record))
    }
}

fn library_supertypes(db: &dyn TyDatabase, scope: &hir::ResolutionScope, ty: Ty) -> Vec<Ty> {
    let TyKind::Reference { args, .. } = ty.kind(db) else {
        return Vec::new();
    };
    let Some(hir::Resolved::Library(resolved)) =
        crate::jvm::resolve::reference_class(db, scope, &ty)
    else {
        return Vec::new();
    };
    let object = Ty::reference(db, "java.lang.Object", Vec::new());
    let is_interface = hir::class_record(db, &resolved)
        .map(|record| match &*record {
            hir::ClassOrModuleRecord::Class(class) => {
                JvmAccessFlags::from_bits_retain(class.flags).is_interface()
            }
            hir::ClassOrModuleRecord::Module(_) => false,
        })
        .unwrap_or(false);
    let mut out = if let Some(info) =
        hir::class_generic_info(db, &hir::Resolved::Library(resolved.clone()))
    {
        if info.type_params.is_empty() || !args.is_empty() {
            let interner = &db.hir_state().interner;
            let owner = Name::new(interner.resolve(&resolved.entry.fqn));
            let class_names: Vec<Name> = info
                .type_params
                .iter()
                .map(|tp| Name::new(interner.resolve(&tp.name)))
                .collect();
            let binding = crate::jvm::resolve::library_class_binding(&owner, &class_names, args);
            let ctx = crate::jvm::resolve::LibrarySignature::class(&owner);
            let instantiate = |tyref: &hir::TypeRef<hir::Symbol>| {
                crate::jvm::resolve::ty_from_library_signature(db, tyref, &ctx)
                    .substitute(db, &binding)
            };
            let mut out = Vec::new();
            if let Some(super_class) = &info.super_class {
                out.push(instantiate(super_class));
            }
            out.extend(info.interfaces.iter().map(instantiate));
            out
        } else {
            let interner = &db.hir_state().interner;
            hir::super_types(db, &hir::Resolved::Library(resolved))
                .into_iter()
                .map(|fqn| Ty::reference(db, Name::new(interner.resolve(&fqn)), Vec::new()))
                .collect()
        }
    } else {
        let interner = &db.hir_state().interner;
        hir::super_types(db, &hir::Resolved::Library(resolved))
            .into_iter()
            .map(|fqn| Ty::reference(db, Name::new(interner.resolve(&fqn)), Vec::new()))
            .collect()
    };
    if is_interface && !out.contains(&object) {
        out.push(object);
    }
    out
}
