//! The project-scoped symbol index end to end: a record written for a file's
//! text answers `file_symbols`, `file_package` and `file_facade_class` without
//! lowering the file, and an edit retires the old record.

// The shared fixture module carries helpers this target does not use (the
// classfile/jar builders belong to the other test targets).
#[allow(dead_code)]
mod common;

use common::{Root, RootFile, build_with_project_cache, fixture, main_source_set};
use vfs::{AbsPathBuf, FileId};

const A: &str = "package p;\n\npublic class A {\n    int f;\n}\n";
const A_EDITED: &str = "package p;\n\npublic class A {\n    int f;\n    int g;\n}\n";

fn file(id: u32, path: &'static str, text: &'static str) -> RootFile {
    RootFile {
        id: FileId::from_raw(id),
        path,
        text,
    }
}

#[test]
fn store_answers_the_per_file_queries() {
    let (_dir, root) = fixture();
    let roots = [Root {
        source_set: main_source_set(),
        files: vec![
            file(1, "/src/p/A.java", A),
            file(2, "/src/p/B.kt", "package p;\n\nfun b() {}\n"),
        ],
        classpath: Vec::new(),
    }];
    let workspace_root = AbsPathBuf::assert_utf8(root.as_std_path().to_owned());
    let mut db = build_with_project_cache(&roots, &[], workspace_root);

    let store = jvm_index::project_index_store(&db).expect("project cache is active");
    assert!(store.is_enabled());

    // Write each file's record as the eager pass would.
    for id in [1u32, 2] {
        let file_id = FileId::from_raw(id);
        let record = jvm_index::compute_file_index(&db, file_id).expect("a Java/Kotlin record");
        let path = jvm_index::file_abs_path(&db, file_id).expect("a source-root path");
        store
            .write_file(record.language, record.content_hash, &record)
            .unwrap();
        store
            .write_path(&path, record.language, record.content_hash)
            .unwrap();
    }

    // The per-file queries now read the store: `file_symbols_query` was not
    // computed yet (only the language's own index was, while building the
    // record), so this is the store-served answer.
    for id in [1u32, 2] {
        let file_id = FileId::from_raw(id);
        let lowered = jvm_index::compute_file_index(&db, file_id).unwrap();
        let expected: Vec<jvm_index::SourceSymbol> = lowered
            .symbols
            .iter()
            .filter_map(|symbol| symbol.to_symbol())
            .collect();
        let served: Vec<jvm_index::SourceSymbol> = jvm_index::file_symbols(&db, file_id)
            .iter()
            .cloned()
            .collect();
        assert_eq!(served, expected, "symbols of file {id}");
        assert_eq!(
            jvm_index::file_package(&db, file_id).as_str(),
            lowered.package
        );
        assert_eq!(
            jvm_index::file_facade_class(&db, file_id)
                .as_ref()
                .map(|name| name.as_str().to_owned()),
            lowered.facade
        );
    }

    // A .kt file has a synthesized facade; the test would be vacuous without it.
    assert!(jvm_index::file_facade_class(&db, FileId::from_raw(2)).is_some());
}

#[test]
fn an_edit_retires_the_old_record() {
    let (_dir, root) = fixture();
    let roots = [Root {
        source_set: main_source_set(),
        files: vec![file(1, "/src/p/A.java", A)],
        classpath: Vec::new(),
    }];
    let workspace_root = AbsPathBuf::assert_utf8(root.as_std_path().to_owned());
    let mut db = build_with_project_cache(&roots, &[], workspace_root);
    let store = jvm_index::project_index_store(&db).unwrap();

    let file_id = FileId::from_raw(1);
    let path = jvm_index::file_abs_path(&db, file_id).unwrap();
    let old = jvm_index::compute_file_index(&db, file_id).unwrap();
    store
        .write_file(old.language, old.content_hash, &old)
        .unwrap();
    store
        .write_path(&path, old.language, old.content_hash)
        .unwrap();
    assert_eq!(
        jvm_index::file_symbols(&db, file_id).len(),
        jvm_index::compute_file_index(&db, file_id)
            .unwrap()
            .symbols
            .len()
    );

    // Edit the file and refresh the index as the incremental pass does.
    db.edit_file(file_id, A_EDITED);
    let new = jvm_index::compute_file_index(&db, file_id).unwrap();
    assert_ne!(new.content_hash, old.content_hash);
    store.delete_file(old.language, old.content_hash).unwrap();
    store
        .write_file(new.language, new.content_hash, &new)
        .unwrap();
    store
        .write_path(&path, new.language, new.content_hash)
        .unwrap();

    // The old key is gone; the edited text is served.
    assert_eq!(store.read_file(old.language, old.content_hash), None);
    assert_eq!(
        store.read_file(new.language, new.content_hash),
        Some(new.clone())
    );
    let served: Vec<jvm_index::SourceSymbol> = jvm_index::file_symbols(&db, file_id)
        .iter()
        .cloned()
        .collect();
    let expected: Vec<jvm_index::SourceSymbol> =
        new.symbols.iter().filter_map(|s| s.to_symbol()).collect();
    assert_eq!(served, expected);
    assert!(served.iter().any(|symbol| symbol.name.as_str() == "p.A.g"));
}
