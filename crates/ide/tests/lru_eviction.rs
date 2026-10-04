//! Tests that the LRU-bound per-file analysis memos are actually evicted by
//! [`ide::AnalysisHost::trigger_lru_eviction`]. Salsa evicts `lru`-marked
//! queries only when that method is called, so without it the bounds on the
//! parse/item/body/type/report memos are inert.

use std::collections::HashMap;

use triomphe::Arc;

use ide::{AnalysisHost, Change, Classpath, ProjectGraphData, SourceSetId};
use ide_db::base_db::{SourceRoot, SourceRootId};
use vfs::{AbsPathBuf, FileId, VfsPath, file_set::FileSet};

/// The capacity of `item_tree_query` (see `hir_def::db::item_tree_query`): the
/// query used as the observable probe below.
const ITEM_TREE_LRU: usize = 512;

fn main_source_set(project: u32) -> SourceSetId {
    SourceSetId {
        project: project_model::ProjectId(project),
        kind: project_model::SourceSetKind::Main,
    }
}

/// A host whose single source set holds `files` as `(raw_file_id, text)`.
fn build(files: &[(u32, String)]) -> (AnalysisHost, HashMap<u32, FileId>) {
    let mut host = AnalysisHost::new();
    let mut change = Change::default();
    let mut file_set = FileSet::default();
    let mut ids = HashMap::new();
    for (raw_id, text) in files {
        let file_id = FileId::from_raw(*raw_id);
        ids.insert(*raw_id, file_id);
        file_set.insert(
            file_id,
            VfsPath::from(AbsPathBuf::assert_utf8(
                format!("/src/p/C{raw_id}.java").into(),
            )),
        );
        change.change_file(file_id, Some(text.clone()));
    }
    change.set_roots(vec![SourceRoot::new(file_set)]);
    let mut data = ProjectGraphData::default();
    data.source_root_to_source_set
        .insert(SourceRootId(0), main_source_set(0));
    data.source_sets.insert(
        main_source_set(0),
        Arc::new(Classpath {
            entries: Vec::new(),
        }),
    );
    change.set_project_graph(data);
    host.apply_change(change);
    (host, ids)
}

#[test]
fn triggering_eviction_drops_the_least_recently_used_item_tree() {
    // `ITEM_TREE_LRU + 1` files: warming one more than the capacity makes the
    // first file the least recently used, so the next eviction drops it.
    let count = ITEM_TREE_LRU + 1;
    let files: Vec<(u32, String)> = (0..count as u32)
        .map(|i| {
            let id = i + 1;
            (id, format!("package p;\npublic class C{id} {{}}\n"))
        })
        .collect();
    let (mut host, ids) = build(&files);

    let first = ids[&1];
    let before = jvm_index::file_item_tree(host.raw_database(), first);
    // A repeated query is served from the memo: the very same `Arc`.
    let repeat = jvm_index::file_item_tree(host.raw_database(), first);
    assert!(
        Arc::ptr_eq(&before, &repeat),
        "a memoized item tree must be reused, not recomputed"
    );

    // Warm every other file so file 1 becomes the least recently used entry.
    for id in 2..=count as u32 {
        let _ = jvm_index::file_item_tree(host.raw_database(), ids[&id]);
    }

    // No snapshot may be held across this call (it blocks until snapshots drop).
    host.trigger_lru_eviction();

    // `before` is still alive, so a recomputation cannot alias its allocation:
    // a distinct pointer proves the memo was evicted and the query re-ran.
    let after = jvm_index::file_item_tree(host.raw_database(), first);
    assert!(
        !Arc::ptr_eq(&before, &after),
        "the least recently used item tree was not evicted"
    );
}
