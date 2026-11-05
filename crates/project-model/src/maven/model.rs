use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct MavenWorkspace {
    pub workspace_name: String,
    pub projects: Vec<MavenProject>,
}

#[derive(Debug, Deserialize)]
pub struct MavenProject {
    pub path: String,
    pub name: String,
    pub project_dir: PathBuf,
    /// Absolute path of the module's main compile output (`target/classes`).
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
    /// Absolute path of the module's test compile output (`target/test-classes`).
    #[serde(default)]
    pub test_output_dir: Option<PathBuf>,
    pub source_roots: Vec<PathBuf>,
    pub test_roots: Vec<PathBuf>,
    pub resource_roots: Vec<PathBuf>,
    pub generated_roots: Vec<PathBuf>,
    pub compile_classpath: Vec<MavenClasspathEntry>,
    pub test_classpath: Vec<MavenClasspathEntry>,
    pub java_language_version: Option<String>,
    pub java_language_preview: Option<bool>,
    /// The explicit `maven.compiler.release` of the project, when it set one
    /// (`javac --release N`, [JEP 247](https://openjdk.org/jeps/247)).
    /// `maven.compiler.source` alone does not select a platform view, so it
    /// never fills this.
    pub java_release: Option<u8>,
    pub java_home: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum MavenClasspathEntry {
    #[serde(rename = "project")]
    Project { path: String, source_set: String },
    #[serde(rename = "jar")]
    Jar {
        path: PathBuf,
        origin: String, // 'coordinate' or 'flat-file'
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An export whose compile roots carry the Kotlin directory the export adds
    /// when the Kotlin plugin did not register it (`addConventionalRoot`).
    const EXPORT: &str = r#"
    {
      "workspace_name": "app",
      "projects": [
        {
          "path": ":app",
          "name": "app",
          "project_dir": "/w/app",
          "output_dir": "/w/app/target/classes",
          "test_output_dir": "/w/app/target/test-classes",
          "source_roots": [
            "/w/app/src/main/java",
            "/w/app/src/main/kotlin"
          ],
          "test_roots": [
            "/w/app/src/test/java",
            "/w/app/src/test/kotlin"
          ],
          "resource_roots": [],
          "generated_roots": [],
          "compile_classpath": [],
          "test_classpath": [],
          "java_language_version": null,
          "java_release": null,
          "java_language_preview": null,
          "java_home": null
        }
      ]
    }
    "#;

    #[test]
    fn kotlin_source_roots_survive_the_export() {
        let workspace: MavenWorkspace =
            serde_json::from_str(EXPORT).expect("the export deserializes");
        let project = &workspace.projects[0];
        assert_eq!(
            project.source_roots,
            vec![
                PathBuf::from("/w/app/src/main/java"),
                PathBuf::from("/w/app/src/main/kotlin"),
            ]
        );
        assert_eq!(
            project.test_roots,
            vec![
                PathBuf::from("/w/app/src/test/java"),
                PathBuf::from("/w/app/src/test/kotlin"),
            ]
        );
    }

    #[test]
    fn output_dirs_survive_the_export() {
        let workspace: MavenWorkspace = serde_json::from_str(EXPORT).unwrap();
        let app = &workspace.projects[0];
        assert_eq!(
            app.output_dir.as_deref(),
            Some(std::path::Path::new("/w/app/target/classes"))
        );
        assert_eq!(
            app.test_output_dir.as_deref(),
            Some(std::path::Path::new("/w/app/target/test-classes"))
        );
    }

    #[test]
    fn missing_output_dirs_default_to_none() {
        // 旧版 sidecar 的 JSON 没有这两个字段，必须能反序列化
        let json = EXPORT
            .replace(r#""output_dir": "/w/app/target/classes","#, "")
            .replace(r#""test_output_dir": "/w/app/target/test-classes","#, "");
        let workspace: MavenWorkspace = serde_json::from_str(&json).unwrap();
        assert!(workspace.projects[0].output_dir.is_none());
        assert!(workspace.projects[0].test_output_dir.is_none());
    }
}
