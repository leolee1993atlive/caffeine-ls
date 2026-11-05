# Caffeine-ls fork: bytecode-mirror fallback

This is a fork of [cubewhy/caffeine-ls](https://github.com/cubewhy/caffeine-ls).
The upstream `README.md` is unchanged; this file documents what the fork adds and
where its limits are.

## What it adds

Resolution of members and types emitted by **annotation processors** — Lombok
getters, builders and constructors, MapStruct implementations, and other APT
output — **without running the processors**. The server reads each source set's
own build output (Maven `target/classes` / `target/test-classes`, Eclipse `bin`)
as bytecode and uses it as a fallback, so the source files can stay in their
pre-processing form (no generated sources required).

## How it works

- **Build-tool export.** The Maven sidecar mojo exports `output_dir` /
  `test_output_dir` for every reactor module; the Eclipse runner takes the
  `.classpath` `output` entry (`bin`).
- **ClassDir index.** Each output directory is indexed as
  `LibraryKind::ClassDir` from its `.class` files, with a stub cache keyed by
  path plus the directory fingerprint (file count, newest mtime, total bytes).
- **Mirror registration.** The index is registered per source set in
  `mirror_libraries`, deliberately *off* the regular classpath, so it can never
  shadow a source declaration or a real classpath class.
- **Merge/type fallback.** For a source receiver, source members win and the
  mirror only adds signatures the source does not declare, but only when the
  `.class` file's mtime is **>=** the source file's mtime. A type that exists
  nowhere source-side is resolved from the mirror last, after source and
  classpath misses (so it cannot shadow either one).

## Requirements and limitations

- **At least one successful compile is required** (`mvn test-compile`, or an
  Eclipse build) so the output directories exist. A project that has never been
  compiled has no mirror to read.
- **A stale or missing mirror disables the member fallback** (nothing is
  invented): if the source file is newer than its `.class`, or the class file is
  absent, source members resolve as before. The mirror's contents and its
  registration are captured when the workspace is loaded, so after an external
  rebuild the server may need a workspace reload to pick up new classes; the
  mtime check only suppresses a mirror older than its source.
- **Mirror-only types are trusted without a source counterpart** (for example
  `Person$PersonBuilder`): they exist only in the mirror and are resolved only
  after source and classpath misses, so they cannot shadow a source type. A
  mirror-only type can survive an incremental build after its source is renamed
  or deleted, until a clean build (e.g. `mvn clean`) removes the stale
  `.class`.
- **Generic signatures may lose some detail.** The mirror is a classfile stub;
  exotic generic `@Builder` chains can retain less type information than a
  source-level expansion would.
- **Maven and Eclipse only for now.** Gradle and IDEA source sets currently
  register no mirror (`mirror_dir` is `None`), so the fallback is inert there.
- **The sidecar plugin version must be bumped whenever its Java sources change**
  (currently `0.1.1`). The version is declared in three places that must be
  bumped together: `SIDECAR_VERSION` in
  `crates/project-model/src/maven/sidecar.rs`, `version =` in
  `crates/project-model/src/maven/sidecar/build.gradle.kts`, and `<version>` in
  `crates/project-model/src/maven/sidecar/src/main/resources/META-INF/maven/plugin.xml`
  (a test in `sidecar.rs` enforces the match). Maven prefers an existing GAV in
  `~/.m2` over the embedded plugin repository, so reusing a version leaves a
  stale mojo in place and silently disables the mirror.
- **JDK 8 `jce.jar` / `javax.crypto` gap** is a known pre-existing issue,
  unrelated to this feature (it is not fixed here).

## Acceptance snapshot (measured 2026-11-05)

`resolve+apply` counts `cant.resolve.location` + `cant.apply.symbol`:

| Project | Baseline total | After total | Baseline resolve+apply | After resolve+apply | Reduction |
|---|---:|---:|---:|---:|---:|
| bl-haems | 7220 | 46 | 7193 | 24 | **99.67%** |
| mmis-java-platform | 23145 | 492 | 22794 | 135 | **99.41%** |
| medical-dispute-platform | 11058 | 4504 | 9754 | 3196 | **67.24%** |

**Two of three projects clear the 95% bar; `medical-dispute-platform` (mdp) does
not, and this README does not claim it does.** The mdp shortfall is dominated by
missing third-party types (530 Lombok-pattern names remain, a 90.9% reduction):
2995 of its 3111 remaining resolve errors are in `jeecg-server-cloud`, whose
Spring / xxl-job / Sentinel / slf4j imports are not present in the workspace or
its classpath (`doesnt.exist` stays unchanged at 843). The mirror cannot create
a type that was never compiled anywhere.

## Regression guarantees

- `../demo-bad`: **21 diagnostics, unchanged** — the same 21-diagnostic LSP set
  as the pre-feature output (javac itself reports 18 errors: 17 attribution + 1
  parse; the LSP set also includes JDT's cascaded messages).
- `../demo-good`: **clean** (`summary: no diagnostics`).
- Offline fixture tests in `crates/caffeine-ls/tests/mirror_fallback.rs`:
  a fresh mirror resolves Lombok-style members/types cleanly, and a stale mirror
  (`src` newer than `bin`) disables the fallback and returns the expected errors.

## Testing note

The full `cargo test --workspace` cannot compile on Windows because
`crates/caffeine-ls/tests/lsp_snapshots.rs` uses `std::os::unix`; this is
pre-existing and unrelated to the mirror. What does run:

```
cargo test -p caffeine-ls --test mirror_fallback
cargo test --workspace --lib --no-fail-fast
cargo test -p caffeine-ls --test cli_workspace_loading
```

All library suites pass except 7 pre-existing Windows path-fixture failures
(5 in the `caffeine-ls` lib, 2 in `jvm_index`); `mirror_fallback` is 2/2.

## License

GPL-3.0, same as upstream (see `LICENSE`).
