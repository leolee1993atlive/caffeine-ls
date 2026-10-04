//! Project-scoped symbol index: a per-file record of the declarations a
//! language frontend lowers, persisted under `<root>/.caffeine-ls/index/v1`.
//!
//! The per-file queries [`crate::db::file_symbols_query`],
//! [`crate::db::file_package_query`] and [`crate::db::file_facade_class`] read
//! this store when the file text they are asked about hashes to a persisted
//! record, so an unchanged file is answered without lowering it or building its
//! item tree at all — a cold session re-reads the index instead of re-parsing
//! every workspace file.
//!
//! The store is keyed by a *stable* content hash of the file's text
//! ([`content_hash`]) plus the language, so a record is never served for text
//! it was not written from; the text hash and the format version are carried in
//! the payload too, so a hash collision or a foreign layout answers as a miss
//! rather than as a wrong declaration set.
//!
//! A second kind of record maps a file's absolute path to the file key written
//! for it, which is what lets an edit or a delete retire the previous key
//! without scanning the whole store (LMDB never shrinks on its own).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use base_db::LanguageKind;
use heed::{Database, Env, EnvOpenOptions, types::Bytes};
use hir_expand::{arena::ArenaId, ids::ItemId, name::Name};
use parking_lot::Mutex;
use postcard::{from_bytes, to_allocvec};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::symbol_index::{SourceSymbol, SourceSymbolKind};

/// Bumped whenever a lowered item's arena index could change meaning (any
/// change to the language lowering's allocation order): a stale arena index is
/// then never read back.
pub const PROJECT_INDEX_FORMAT_VERSION: u32 = 1;

/// Virtual size of the memory map. LMDB requires an upper bound up front; the
/// file itself only grows with actual data (sparse on Linux).
const INDEX_MAP_SIZE: usize = 1 << 30;

/// Reader slots. LMDB's default is 126, and the eager pass and the chunked
/// diagnostics run many short parallel reads; 1024 avoids `MDB_READERS_FULL`.
const INDEX_MAX_READERS: u32 = 1024;

/// `[language u8][kind u8][hash u64 BE]`.
const KEY_LEN: usize = 10;
const KIND_FILE: u8 = 1;
const KIND_PATH: u8 = 2;

/// Stable content hash of a file's text: `xxh3_64` over the UTF-8 bytes.
/// Deterministic across processes, platforms and toolchain versions, unlike
/// `std::hash::DefaultHasher`.
pub fn content_hash(text: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(text.as_bytes())
}

/// A persisted declaration: the [`SourceSymbol`] fields with the interned
/// [`Name`] and arena-relative [`ItemId`] flattened to plain data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedSymbol {
    pub name: String,
    pub kind: u8,
    pub public: bool,
    pub item: u32,
}

impl PersistedSymbol {
    pub(crate) fn from_symbol(symbol: &SourceSymbol) -> Self {
        Self {
            name: symbol.name.as_str().to_owned(),
            kind: symbol.kind.as_u8(),
            public: symbol.public,
            item: symbol.item.0.0,
        }
    }

    /// The in-memory symbol, `None` for a record that cannot name one: an
    /// unknown kind, or an empty name.
    pub fn to_symbol(&self) -> Option<SourceSymbol> {
        if self.name.is_empty() {
            return None;
        }
        Some(SourceSymbol {
            name: Name::new(&self.name),
            item: ItemId(ArenaId(self.item)),
            kind: SourceSymbolKind::from_u8(self.kind)?,
            public: self.public,
        })
    }
}

/// The persisted record of one source file: its package, its synthesized
/// facade class and its declarations, keyed by the text they were lowered from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileIndexBlob {
    pub format_version: u32,
    pub content_hash: u64,
    pub language: u8,
    pub package: String,
    pub facade: Option<String>,
    pub symbols: Vec<PersistedSymbol>,
}

impl FileIndexBlob {
    /// The declarations as in-memory symbols, dropping records that cannot name
    /// one.
    pub fn symbols(&self) -> impl Iterator<Item = SourceSymbol> + '_ {
        self.symbols.iter().filter_map(PersistedSymbol::to_symbol)
    }
}

/// Absolute path → the file key written for it, so an edit or delete retires
/// the old entry without a full live-set sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathEntryBlob {
    pub format_version: u32,
    pub path: String,
    pub language: u8,
    pub content_hash: u64,
}

/// The language byte code persisted for a file, `None` for a language the store
/// does not index.
pub fn language_code(kind: LanguageKind) -> Option<u8> {
    Some(match kind {
        LanguageKind::Java => 0,
        LanguageKind::Kotlin => 1,
        LanguageKind::KotlinScript => 2,
        LanguageKind::Unknown => return None,
    })
}

struct StoreInner {
    env: Env,
    db: Database<Bytes, Bytes>,
}

/// Handle to the project symbol index.
///
/// The store starts disabled and is pointed at a directory once via
/// [`ProjectIndexStore::open_at`]; the environment is created lazily on the
/// first use, so constructing or cloning a store never touches the filesystem
/// until then. Every method degrades gracefully to a no-op when the store is
/// disabled or failed to open.
///
/// Cheap to clone: all clones share one environment.
#[derive(Clone, Default)]
pub struct ProjectIndexStore {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// `None` = not opened yet; `Some(None)` = disabled / failed.
    inner: OnceLock<Option<Arc<StoreInner>>>,
    /// Where to open the environment; `None` keeps the store memory-only.
    dir: Mutex<Option<PathBuf>>,
}

impl std::fmt::Debug for ProjectIndexStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.shared.inner.get() {
            Some(Some(_)) => f.write_str("ProjectIndexStore(open)"),
            Some(None) => f.write_str("ProjectIndexStore(disabled)"),
            None => f.write_str("ProjectIndexStore(unopened)"),
        }
    }
}

impl ProjectIndexStore {
    /// Points the store at `dir`. Must be called before the first use; later
    /// calls are ignored (the environment is already decided).
    pub fn open_at(&self, dir: PathBuf) {
        if self.shared.inner.get().is_some() {
            tracing::debug!("project index already initialized; ignoring new cache dir");
            return;
        }
        *self.shared.dir.lock() = Some(dir);
    }

    /// The directory this store was pointed at, if any.
    pub fn dir(&self) -> Option<PathBuf> {
        self.shared.dir.lock().clone()
    }

    /// Whether the store is available (used by callers and tests).
    pub fn is_enabled(&self) -> bool {
        self.ensure_open().is_some()
    }

    fn ensure_open(&self) -> Option<&Arc<StoreInner>> {
        self.shared
            .inner
            .get_or_init(|| {
                let dir = self.shared.dir.lock().clone()?;
                match Self::try_open(&dir) {
                    Ok(inner) => Some(inner),
                    Err(err) => {
                        tracing::error!(
                            dir = %dir.display(),
                            "failed to open project symbol index; continuing memory-only: {err:#}"
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    fn try_open(dir: &Path) -> anyhow::Result<Arc<StoreInner>> {
        std::fs::create_dir_all(dir)?;
        // SAFETY: the environment is used from multiple threads but never
        // across fork(); all accesses go through short-lived transactions.
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(INDEX_MAP_SIZE)
                .max_readers(INDEX_MAX_READERS)
                .open(dir)
        }?;
        let mut wtxn = env.write_txn()?;
        let db = env.create_database::<Bytes, Bytes>(&mut wtxn, None)?;
        wtxn.commit()?;
        Ok(Arc::new(StoreInner { env, db }))
    }

    /// The file record for `(language, content_hash)`, `None` when absent or
    /// written for different text or by a foreign layout.
    pub fn read_file(&self, language: u8, content_hash: u64) -> Option<FileIndexBlob> {
        let store = self.ensure_open()?;
        let rtxn = store.env.read_txn().ok()?;
        let bytes = store
            .db
            .get(&rtxn, &file_key(language, content_hash))
            .ok()??;
        let blob: FileIndexBlob = from_bytes(bytes).ok()?;
        (blob.format_version == PROJECT_INDEX_FORMAT_VERSION
            && blob.content_hash == content_hash
            && blob.language == language)
            .then_some(blob)
    }

    /// Writes (or replaces) the file record for `(language, content_hash)`.
    pub fn write_file(
        &self,
        language: u8,
        content_hash: u64,
        blob: &FileIndexBlob,
    ) -> anyhow::Result<()> {
        let Some(store) = self.ensure_open() else {
            anyhow::bail!("project symbol index is disabled");
        };
        let mut wtxn = store.env.write_txn()?;
        let payload = to_allocvec(blob)
            .map_err(|err| anyhow::anyhow!("failed to serialize file index blob: {err}"))?;
        store
            .db
            .put(&mut wtxn, &file_key(language, content_hash), &payload)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Deletes one file record (already superseded by a new hash for the same
    /// path).
    pub fn delete_file(&self, language: u8, content_hash: u64) -> anyhow::Result<()> {
        let Some(store) = self.ensure_open() else {
            anyhow::bail!("project symbol index is disabled");
        };
        let mut wtxn = store.env.write_txn()?;
        store
            .db
            .delete(&mut wtxn, &file_key(language, content_hash))?;
        wtxn.commit()?;
        Ok(())
    }

    /// The file key recorded for `path`, `None` when absent. A hash collision
    /// is a miss: the recorded path is compared to the requested one.
    pub fn read_path(&self, path: &str) -> Option<PathEntryBlob> {
        let store = self.ensure_open()?;
        let rtxn = store.env.read_txn().ok()?;
        let bytes = store.db.get(&rtxn, &path_key(path)).ok()??;
        let blob: PathEntryBlob = from_bytes(bytes).ok()?;
        (blob.format_version == PROJECT_INDEX_FORMAT_VERSION && blob.path == path).then_some(blob)
    }

    /// Records `path` as written for `(language, content_hash)`.
    pub fn write_path(&self, path: &str, language: u8, content_hash: u64) -> anyhow::Result<()> {
        let Some(store) = self.ensure_open() else {
            anyhow::bail!("project symbol index is disabled");
        };
        let blob = PathEntryBlob {
            format_version: PROJECT_INDEX_FORMAT_VERSION,
            path: path.to_owned(),
            language,
            content_hash,
        };
        let mut wtxn = store.env.write_txn()?;
        let payload = to_allocvec(&blob)
            .map_err(|err| anyhow::anyhow!("failed to serialize path entry blob: {err}"))?;
        store.db.put(&mut wtxn, &path_key(path), &payload)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Forgets `path` (the file was deleted or is no longer part of the index).
    pub fn delete_path(&self, path: &str) -> anyhow::Result<()> {
        let Some(store) = self.ensure_open() else {
            anyhow::bail!("project symbol index is disabled");
        };
        let mut wtxn = store.env.write_txn()?;
        store.db.delete(&mut wtxn, &path_key(path))?;
        wtxn.commit()?;
        Ok(())
    }

    /// Deletes every record that a full pass left behind: a file record whose
    /// `(language, hash)` is not in `live`, and a path record that names such a
    /// file (so a deleted file's mapping is swept too). Returns the number of
    /// records removed.
    pub fn prune(&self, live: &FxHashSet<(u8, u64)>) -> anyhow::Result<usize> {
        let Some(store) = self.ensure_open() else {
            anyhow::bail!("project symbol index is disabled");
        };
        let rtxn = store.env.read_txn()?;
        let mut stale: Vec<[u8; KEY_LEN]> = Vec::new();
        {
            let mut iter = store.db.iter(&rtxn)?;
            while let Some((key, value)) = iter.next().transpose()? {
                if key.len() != KEY_LEN {
                    continue;
                }
                let alive = match key[1] {
                    KIND_FILE => {
                        let language = key[0];
                        let hash =
                            u64::from_be_bytes(key[2..].try_into().expect("hash is 8 bytes"));
                        live.contains(&(language, hash))
                    }
                    KIND_PATH => from_bytes::<PathEntryBlob>(value).ok().is_some_and(|blob| {
                        blob.format_version == PROJECT_INDEX_FORMAT_VERSION
                            && live.contains(&(blob.language, blob.content_hash))
                    }),
                    _ => true,
                };
                if !alive {
                    stale.push(key.try_into().expect("key is KEY_LEN bytes"));
                }
            }
        }
        drop(rtxn);
        if stale.is_empty() {
            return Ok(0);
        }
        let mut wtxn = store.env.write_txn()?;
        for key in &stale {
            store.db.delete(&mut wtxn, key.as_slice())?;
        }
        wtxn.commit()?;
        Ok(stale.len())
    }
}

fn file_key(language: u8, hash: u64) -> [u8; KEY_LEN] {
    let mut key = [0; KEY_LEN];
    key[0] = language;
    key[1] = KIND_FILE;
    key[2..].copy_from_slice(&hash.to_be_bytes());
    key
}

fn path_key(path: &str) -> [u8; KEY_LEN] {
    let mut key = [0; KEY_LEN];
    key[0] = 0;
    key[1] = KIND_PATH;
    key[2..].copy_from_slice(&content_hash(path).to_be_bytes());
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, ProjectIndexStore) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = ProjectIndexStore::default();
        store.open_at(dir.path().to_owned());
        (dir, store)
    }

    fn blob(hash: u64) -> FileIndexBlob {
        FileIndexBlob {
            format_version: PROJECT_INDEX_FORMAT_VERSION,
            content_hash: hash,
            language: 0,
            package: "pkg".to_owned(),
            facade: Some("PkgKt".to_owned()),
            symbols: vec![PersistedSymbol {
                name: "pkg.A".to_owned(),
                kind: SourceSymbolKind::Class.as_u8(),
                public: true,
                item: 7,
            }],
        }
    }

    #[test]
    fn file_record_round_trips() {
        let (_dir, store) = temp_store();
        assert!(store.is_enabled());
        store.write_file(0, 0xabcd, &blob(0xabcd)).unwrap();
        assert_eq!(store.read_file(0, 0xabcd), Some(blob(0xabcd)));
    }

    #[test]
    fn content_hash_and_language_must_match() {
        let (_dir, store) = temp_store();
        store.write_file(0, 1, &blob(1)).unwrap();
        // A different hash or a different language must not serve the record.
        assert_eq!(store.read_file(0, 2), None);
        assert_eq!(store.read_file(1, 1), None);
    }

    #[test]
    fn foreign_format_version_is_a_miss() {
        let (_dir, store) = temp_store();
        let mut stale = blob(1);
        stale.format_version = PROJECT_INDEX_FORMAT_VERSION + 1;
        store.write_file(0, 1, &stale).unwrap();
        assert_eq!(store.read_file(0, 1), None);
    }

    #[test]
    fn disabled_store_is_a_no_op() {
        let store = ProjectIndexStore::default();
        assert!(!store.is_enabled());
        assert_eq!(store.read_file(0, 1), None);
        assert_eq!(store.read_path("/p/A.java"), None);
        assert!(store.write_file(0, 1, &blob(1)).is_err());
        assert!(store.write_path("/p/A.java", 0, 1).is_err());
        assert!(store.prune(&FxHashSet::default()).is_err());
    }

    #[test]
    fn path_collision_is_a_miss() {
        let (_dir, store) = temp_store();
        store.write_path("/p/A.java", 0, 1).unwrap();
        assert_eq!(store.read_path("/p/B.java"), None);
        assert_eq!(store.read_path("/p/A.java").unwrap().content_hash, 1);
    }

    #[test]
    fn delete_path_and_file_retire_an_entry() {
        let (_dir, store) = temp_store();
        store.write_file(0, 1, &blob(1)).unwrap();
        store.write_path("/p/A.java", 0, 1).unwrap();
        store.delete_file(0, 1).unwrap();
        store.delete_path("/p/A.java").unwrap();
        assert_eq!(store.read_file(0, 1), None);
        assert_eq!(store.read_path("/p/A.java"), None);
    }

    #[test]
    fn prune_removes_records_not_in_the_live_set() {
        let (_dir, store) = temp_store();
        store.write_file(0, 1, &blob(1)).unwrap();
        store.write_file(0, 2, &blob(2)).unwrap();
        store.write_path("/p/A.java", 0, 1).unwrap();
        store.write_path("/p/B.java", 0, 2).unwrap();

        // Only 1 is live: the record and its path mapping for 2 are swept.
        let live: FxHashSet<(u8, u64)> = [(0u8, 1u64)].into_iter().collect();
        assert_eq!(store.prune(&live).unwrap(), 2);
        assert_eq!(store.read_file(0, 1), Some(blob(1)));
        assert_eq!(store.read_file(0, 2), None);
        assert_eq!(store.read_path("/p/A.java").unwrap().content_hash, 1);
        assert_eq!(store.read_path("/p/B.java"), None);
    }
}
