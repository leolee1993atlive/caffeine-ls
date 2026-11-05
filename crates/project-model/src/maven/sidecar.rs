//! The Maven export sidecar plugin.
//!
//! The plugin jar is built by `build.rs` (from the colocated Gradle project in
//! `src/maven/sidecar/`) and embedded into the binary. At workspace-import time
//! [`ensure_extracted`] materializes the jar, a matching pom, and a
//! `settings.xml` into the system temp directory. The `settings.xml` wires a
//! `file://` plugin repository pointing at the extracted jar, so the sidecar is
//! resolvable by Maven without polluting the user's `~/.m2` repository.
use std::fs;
use std::path::{Path, PathBuf};

pub const SIDECAR_GROUP: &str = "org.cubewhy.caffeine_ls";
pub const SIDECAR_ARTIFACT: &str = "caffeine-ls-maven-sidecar";
// Bump this whenever the sidecar Java sources change. The version is declared
// in three places that must move together: this constant, the `version =` in
// `src/maven/sidecar/build.gradle.kts`, and `<version>` in
// `src/main/resources/META-INF/maven/plugin.xml` (asserted by the test below).
// Maven resolves an existing GAV from `~/.m2` before consulting the embedded
// `file://` plugin repository, so reusing a version leaves a stale jar in
// place and the new mojo code (e.g. `output_dir`) never runs, silently
// disabling the mirror.
pub const SIDECAR_VERSION: &str = "0.1.1";

/// The embedded sidecar plugin jar.
pub fn sidecar_jar_bytes() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/caffeine-ls-maven-sidecar.jar"))
}

fn sidecar_dir() -> PathBuf {
    std::env::temp_dir()
        .join("caffeine-ls")
        .join(format!("{SIDECAR_ARTIFACT}-{SIDECAR_VERSION}"))
}

/// Ensures the embedded sidecar jar, its pom, and a `settings.xml` exposing it
/// as a `file://` plugin repository are present in the temp directory.
/// Returns the sidecar directory (which contains `settings.xml`).
pub fn ensure_extracted() -> anyhow::Result<PathBuf> {
    let dir = sidecar_dir();

    let repo_dir = dir
        .join("repo")
        .join(SIDECAR_GROUP.replace('.', "/"))
        .join(SIDECAR_ARTIFACT)
        .join(SIDECAR_VERSION);
    fs::create_dir_all(&repo_dir)?;

    let jar_name = format!("{SIDECAR_ARTIFACT}-{SIDECAR_VERSION}.jar");
    fs::write(repo_dir.join(&jar_name), sidecar_jar_bytes())?;
    fs::write(
        repo_dir.join(format!("{SIDECAR_ARTIFACT}-{SIDECAR_VERSION}.pom")),
        sidecar_pom(),
    )?;

    fs::write(dir.join("settings.xml"), settings_xml(&dir.join("repo")))?;

    Ok(dir)
}

fn sidecar_pom() -> String {
    format!(
        r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>{SIDECAR_GROUP}</groupId>
  <artifactId>{SIDECAR_ARTIFACT}</artifactId>
  <version>{SIDECAR_VERSION}</version>
  <packaging>maven-plugin</packaging>
</project>
"#
    )
}

fn settings_xml(repo_dir: &Path) -> String {
    format!(
        r#"<settings xmlns="http://maven.apache.org/SETTINGS/1.0.0">
  <profiles>
    <profile>
      <id>caffeine-ls-sidecar</id>
      <activation><activeByDefault>true</activeByDefault></activation>
      <pluginRepositories>
        <pluginRepository>
          <id>caffeine-ls-sidecar-repo</id>
          <url>{}</url>
          <releases><enabled>true</enabled></releases>
          <snapshots><enabled>false</enabled></snapshots>
        </pluginRepository>
      </pluginRepositories>
    </profile>
  </profiles>
</settings>
"#,
        file_url(repo_dir)
    )
}

/// Converts an absolute path to a `file://` URL for Maven repository usage.
fn file_url(path: &Path) -> String {
    let mut path = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        path = format!("/{path}");
    }
    format!("file://{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The version is declared in Rust, Gradle, and the generated plugin
    /// descriptor. Drifting any one of them silently disables the mirror (a
    /// stale GAV in `~/.m2` wins over the embedded repository), so keep all
    /// three in lockstep.
    #[test]
    fn sidecar_version_is_declared_consistently() {
        let sidecar = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/maven/sidecar");

        let gradle = fs::read_to_string(sidecar.join("build.gradle.kts"))
            .expect("sidecar build.gradle.kts should be readable");
        assert!(
            gradle.contains(&format!("version = \"{SIDECAR_VERSION}\"")),
            "build.gradle.kts must declare version = \"{SIDECAR_VERSION}\""
        );

        let plugin_xml =
            fs::read_to_string(sidecar.join("src/main/resources/META-INF/maven/plugin.xml"))
                .expect("sidecar plugin.xml should be readable");
        assert!(
            plugin_xml.contains(&format!("<version>{SIDECAR_VERSION}</version>")),
            "plugin.xml must declare <version>{SIDECAR_VERSION}</version>"
        );
    }
}
