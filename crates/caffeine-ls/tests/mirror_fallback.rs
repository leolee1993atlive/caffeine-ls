//! End-to-end coverage of the bytecode-mirror fallback: a workspace whose
//! source files are the *pre*-annotation-processing form, while `bin/` holds
//! the expanded class files an annotation processor would have generated.
//!
//! The fixture is copied into a temporary directory so the tests can set the
//! `src/` and `bin/` mtimes explicitly — the fallback's freshness gate compares
//! the two, and checkout mtimes are not reproducible.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

use tempfile::TempDir;

const FIXTURE: &str = "tests/fixtures/lombok-mirror";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

/// Recursively copies `from` into `to`, preserving the file bytes only.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Sets the modification time of every file under `root` (recursively).
fn set_tree_mtime(root: &Path, time: SystemTime) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            set_tree_mtime(&path, time);
        } else {
            let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
    }
}

/// The JDK the fixture's JRE container and the language server resolve
/// against: `JAVA_HOME` when it points at a real JDK, else the host's known
/// Dragonwell 21, else `None` (the tests skip).
fn java_home() -> Option<PathBuf> {
    let usable = |path: &Path| path.join("bin").join("javac.exe").exists();
    if let Some(home) = std::env::var_os("JAVA_HOME").map(PathBuf::from)
        && usable(&home)
    {
        return Some(home);
    }
    let fallback = PathBuf::from("C:/Users/limy/.jdks/dragonwell-21.0.11");
    usable(&fallback).then_some(fallback)
}

/// Copies the fixture to a temporary workspace. `mirror_newer` selects the
/// freshness relation the server must observe: `bin/` newer than `src/` when
/// true, stale otherwise.
fn workspace(mirror_newer: bool) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&fixture_dir(), dir.path());

    let now = SystemTime::now();
    let older = now - Duration::from_secs(2 * 60 * 60);
    let (source, mirror) = if mirror_newer {
        (older, now)
    } else {
        (now, older)
    };
    set_tree_mtime(&dir.path().join("src"), source);
    set_tree_mtime(&dir.path().join("bin"), mirror);
    dir
}

fn run_diagnostics(root: &Path, java_home: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_caffeine-ls"))
        .arg("diagnostics")
        .arg(root)
        .arg("--no-progress")
        .arg("--java-home")
        .arg(java_home)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.code().is_some(),
        "caffeine-ls did not exit normally\nstderr:\n{stderr}"
    );
    stdout
}

#[test]
fn lombok_style_members_and_types_resolve_via_mirror() {
    let Some(java_home) = java_home() else {
        eprintln!("skipping: no JDK available (set JAVA_HOME)");
        return;
    };
    // `bin/` is newer than `src/`: the mirror wins and every source-only gap
    // (factory, getters, two-argument constructor, builder type/methods, the
    // generated `log` field) resolves through it.
    let workspace = workspace(true);
    let stdout = run_diagnostics(workspace.path(), &java_home);
    let lower = stdout.to_lowercase();
    assert!(
        !lower.contains("cannot resolve"),
        "mirror fallback failed; diagnostics:\n{stdout}"
    );
    assert!(
        stdout.contains("summary: no diagnostics"),
        "expected a clean fixture; diagnostics:\n{stdout}"
    );
}

#[test]
fn stale_mirror_disables_the_fallback() {
    let Some(java_home) = java_home() else {
        eprintln!("skipping: no JDK available (set JAVA_HOME)");
        return;
    };
    // Sources are newer than `bin/`: the mirror is stale, so the fallback must
    // stay off and the unresolved members reappear.
    let workspace = workspace(false);
    let stdout = run_diagnostics(workspace.path(), &java_home);
    let lower = stdout.to_lowercase();
    assert!(
        lower.contains("cannot resolve"),
        "stale mirror should not resolve members; diagnostics:\n{stdout}"
    );
}
