//! The project-scoped cache directory `<root>/.caffeine-ls`.
//!
//! Unlike the global cache (library stubs, shared by every project), this one
//! belongs to the workspace it sits in: it holds the project symbol index and
//! anything else derived from the project's own sources. Two marker files make
//! the directory self-describing and safe to commit-ignore:
//!
//! * `.gitignore` containing `*`, so `git status` stays clean even in a
//!   repository whose `.gitignore` does not name the directory.
//! * `CACHEDIR.TAG`, the standard marker tools (`tar`, backup software) read to
//!   skip a cache directory.
//!
//! The directory is hidden, so the vfs loader's `standard_filters(true)` walk
//! already excludes it; no loader change is needed.

use std::path::PathBuf;

use vfs::AbsPath;

/// The contents of `.caffeine-ls/.gitignore`.
const GITIGNORE: &str = "*\n";

/// The standard `CACHEDIR.TAG` header.
const CACHEDIR_TAG: &str = concat!(
    "Signature: 8a477f597d28d172789f06886806bc55\n",
    "# This file is a cache directory tag created by caffeine-ls.\n",
    "# For information about cache directory tags see https://bford.info/cachedir/\n",
);

/// `<root>/.caffeine-ls` — the project-scoped cache directory.
pub(crate) fn dir(root: &AbsPath) -> PathBuf {
    PathBuf::from(root.join(".caffeine-ls").as_str())
}

/// Creates the cache directory and its marker files. Idempotent. A read-only
/// or otherwise unwritable filesystem is logged and answered with `None`; the
/// project index then runs memory-only (queries fall back to lowering), which
/// is exactly the behavior without a project cache.
pub(crate) fn prepare(root: &AbsPath) -> Option<PathBuf> {
    let dir = dir(root);
    match write_markers(&dir) {
        Ok(()) => Some(dir),
        Err(err) => {
            tracing::warn!(
                dir = %dir.display(),
                "failed to prepare project cache directory; running memory-only: {err}"
            );
            None
        }
    }
}

fn write_markers(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(".gitignore"), GITIGNORE)?;
    std::fs::write(dir.join("CACHEDIR.TAG"), CACHEDIR_TAG)?;
    Ok(())
}
