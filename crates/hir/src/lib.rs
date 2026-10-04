//! The HIR facade: the workspace index and the type layer as one crate.
//!
//! The index ([`jvm_index`], package `jvm_index`) answers what exists —
//! files, symbols, classpath classes and the classfile-shaped records the
//! stub index keeps. The type layer (`hir-ty`) answers what a declaration
//! or expression means — [`Ty`], the member walk, inference and the
//! structured diagnostics the checks produce. Consumers above both, such as
//! `ide`, depend on this crate and name a single surface.
//!
//! Re-exports only: every path here is the path of the crate that defines
//! it. No new types, no wrappers.

// The index side: what exists.
pub use jvm_index::hir_expand::name::Name;
pub use jvm_index::{
    AnnotationSig, AnnotationValue, CachedMemberParams, ClassEntry, ClassGenericInfo, ClassKind,
    ClassOrModuleRecord, ClassOrModuleStub, ClassRecord, ClassStub, Classpath, ClasspathEntry,
    CtSymIndex, DocIndex, FieldStub, HirDatabase, HirState, JavaDatabase, JavaLanguageLevel,
    JvmDatabase, KotlinDatabase, LibraryId, LibraryIndex, LibraryInfo, LibraryKind,
    LibrarySourceDecl, LibrarySources, MethodStub, ModuleCtx, ModuleDescriptor, ModuleEntry,
    ModuleGraph, ModuleRecord, ModuleStub, NameIndex, ParamData, PrimitiveType, PrimitiveValue,
    ProjectGraph, ProjectGraphData, ProjectId, ResolutionScope, Resolved, ResolvedClass,
    SourceClass, SourceSetId, SourceSetKind, SourceSymbol, SourceSymbolIndex, SourceSymbolKind,
    SourceSymbolRef, Symbol, TypeRef, WorkspaceModuleGraph, cache_member_params,
    cached_member_params, class_generic_info, class_record, classes_in_package, classpath,
    classpath_libraries, ct_sym_class_not_in_release, ct_sym_index, ct_sym_member_not_in_release,
    enable_persistent_stub_cache, file_body_tree, file_docs, file_facade_class, file_facade_source,
    file_item_tree, file_name, file_package_dir, file_path_segments, file_symbols, fqn_resolve,
    is_package_exported, is_package_visible, is_package_visible_from_unnamed, item_doc,
    jdk_builtin_libraries, language_level_for_file, library_decompiles, library_name_index,
    library_source_decl, library_source_for_file, library_source_path, library_sources,
    module_ctx_for_scope, module_descriptor, module_for_class, module_graph,
    module_graph_for_source_set, module_name, module_record, modules_for_package, package_exists,
    project_graph, prune_stub_cache, readable_modules, registered_libraries, release_for_file,
    release_for_source_set, required_modules, resolve_in_libraries, set_project_graph,
    source_class_fqn, source_set_for_file, source_set_fqn_symbols, source_set_package_files,
    source_set_symbols, super_types, warmup_library,
};
pub use jvm_index::{
    ct_sym, db, hir_def, hir_expand, index, lib_source, lmdb_store, loader, modules, project,
    stubs, symbol_index,
};

// The type side: what it means.
pub use hir_ty::java;
pub use hir_ty::jvm;
pub use hir_ty::kotlin;
pub use hir_ty::lang;
pub use hir_ty::range_ctx;
pub use hir_ty::{
    Access, AnnotationTarget, BodyTypes, BoundKind, ClassKey, DeclDiagnostic, DiagLocation,
    FieldData, InvocationContext, InvocationMode, KotlinBodyTypes, KotlinResolvedMember,
    KotlinResolver, KotlinTypeError, MethodData, MethodDisplay, MethodTypeParam, NameResolution,
    PolyArg, ReleaseApi, ResolvedMember, Resolver, Ty, TyData, TyDatabase, TyDisplay, TyKind,
    TypeError, TypeParamDeclaration, TypeVarScope, WildcardBound, abstract_methods, access_context,
    all_methods_for_test, annotation_target, body_types, capture_conversion, class_diagnostics,
    display_kotlin, inferred_lambda_parameter_ty, is_assignable, is_subtype,
    item_annotation_references, item_ty, item_type_references, kotlin_assignable,
    kotlin_body_types, kotlin_class_diagnostics, kotlin_declaration_types, kotlin_declared_members,
    kotlin_initializer_types, kotlin_item_ty, kotlin_script_body_types, kotlin_subtype,
    kotlin_supertypes, kotlin_type_params, least_upper_bound, level_diagnostics, member_set,
    member_set_ignoring_access, method_params, module_diagnostics, pick_field,
    pick_field_ignoring_access, pick_method, record_component_types, resolve_type_name_at,
    resolve_type_ref, scope_for_file, scope_for_test, single_abstract_method, supertypes,
    ty_from_library, ty_from_source, type_param_declaration,
};
