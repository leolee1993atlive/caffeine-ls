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
/// against.
///
/// A set `JAVA_HOME` is authoritative and must point at a usable JDK: a broken
/// environment is a test failure, not a skip, because skipping silently would
/// disable the only automated coverage of the fallback. Only when no
/// `JAVA_HOME` is set at all does the host's known Dragonwell 21 act as a
/// fallback (and only when that is missing too is the test skipped).
fn java_home() -> Option<PathBuf> {
    let usable = |path: &Path| {
        let bin = path.join("bin");
        // `javac.exe` on Windows, plain `javac` on Linux/macOS.
        bin.join("javac").exists() || bin.join("javac.exe").exists()
    };
    match std::env::var_os("JAVA_HOME").map(PathBuf::from) {
        Some(home) => {
            assert!(
                usable(&home),
                "JAVA_HOME is set to `{}` but it contains no bin/javac",
                home.display()
            );
            Some(home)
        }
        None => {
            let fallback = PathBuf::from("C:/Users/limy/.jdks/dragonwell-21.0.11");
            usable(&fallback).then_some(fallback)
        }
    }
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

/// Runs `diagnostics` and returns stdout. `expected_exit` is the documented
/// exit code: `0` (clean) or `1` (findings); anything else — including a
/// panic — fails the test with both streams.
fn run_diagnostics(root: &Path, java_home: &Path, expected_exit: i32) -> String {
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
    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "caffeine-ls exited with {:?}, expected {expected_exit}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
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
    let stdout = run_diagnostics(workspace.path(), &java_home, 0);
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
    // stay off and the unresolved members reappear. Pin the generated members
    // the source does not declare — a gate regression that only disabled, say,
    // constructor merging would otherwise still satisfy a bare
    // `cannot resolve` check.
    let workspace = workspace(false);
    let stdout = run_diagnostics(workspace.path(), &java_home, 1);
    for expected in [
        "Cannot resolve method 'of()'",
        "Cannot resolve method 'getName()'",
        "Cannot resolve method 'builder()'",
        "Cannot resolve method 'getAge()'",
        "Cannot resolve symbol 'log'",
    ] {
        assert!(
            stdout.contains(expected),
            "expected `{expected}` from the stale mirror; diagnostics:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("summary: no diagnostics"),
        "stale mirror must not produce a clean report; diagnostics:\n{stdout}"
    );
}
