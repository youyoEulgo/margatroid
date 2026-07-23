use std::fs;
use std::path::Path;

use base64::Engine;
use margatroid_compose::{CompileOptions, Compiler, DiagnosticCode, ProjectLimits, compile};
use margatroid_protocol::ResourcePackage;
use tempfile::TempDir;

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn project() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory
            .path()
            .join(".margatroid/skills/acme/reviewer/SKILL.md"),
        "project reviewer\n",
    );
    write(
        directory
            .path()
            .join(".margatroid/workflows/acme/review/workflow.toml"),
        "version = 1\n",
    );
    write(
        directory.path().join("margatroid-workspace.yaml"),
        r#"schema_version: 1
workspace:
  name: review-team
  manager: coordinator
agents:
  reviewer:
    image: acme/reviewer@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    skills:
      - acme/reviewer
  coordinator:
    image: acme/coordinator:v1
    skills:
      - acme/reviewer
    workflows:
      - acme/review
"#,
    );
    directory
}

#[test]
fn compiles_scoped_packages_into_a_deterministic_bundle() {
    let project = project();
    let path = project.path().join("margatroid-workspace.yaml");
    let first = compile(&path).unwrap();
    let second = Compiler::default()
        .compile(project.path().join("./margatroid-workspace.yaml"))
        .unwrap();

    assert_eq!(first.bundle(), second.bundle());
    assert_eq!(first.bundle().spec.name.as_str(), "review-team");
    assert_eq!(first.bundle().spec.agents[0].id.as_str(), "coordinator");
    assert_eq!(first.bundle().manifest.entries.len(), 2);
    assert_eq!(first.bundle().resources.len(), 2);

    let normalized = first.normalized().to_json().unwrap();
    assert!(!normalized.contains("content_base64"));
    assert!(!normalized.contains(project.path().to_str().unwrap()));
}

#[test]
fn project_package_overrides_same_named_main_package() {
    let project = project();
    let main = tempfile::tempdir().unwrap();
    write(
        main.path().join("skills/acme/reviewer/SKILL.md"),
        "main reviewer\n",
    );

    let output = Compiler::new(CompileOptions::default().with_main_directory(main.path()))
        .compile(project.path().join("margatroid-workspace.yaml"))
        .unwrap();
    let skill_digest = &output.bundle().manifest.entries[0].digest;
    let bundled = output
        .bundle()
        .resources
        .iter()
        .find(|resource| &resource.digest == skill_digest)
        .unwrap();
    let package = base64::engine::general_purpose::STANDARD
        .decode(&bundled.content_base64)
        .unwrap();
    assert!(
        String::from_utf8(package)
            .unwrap()
            .contains("cHJvamVjdCByZXZpZXdlcgo=")
    );
}

#[test]
fn rejects_unknown_fields_and_missing_manager() {
    let project = tempfile::tempdir().unwrap();
    write(
        project.path().join("margatroid-workspace.yaml"),
        r#"schema_version: 1
workspace:
  manager: missing
  workflows: []
agents:
  coder:
    image: acme/coder:v1
"#,
    );
    let error = Compiler::default()
        .compile(project.path().join("margatroid-workspace.yaml"))
        .unwrap_err();
    let codes: Vec<_> = error
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(codes.contains(&DiagnosticCode::UnknownField));
    assert!(codes.contains(&DiagnosticCode::MissingManager));
}

#[test]
fn rejects_parent_paths_and_digest_mismatches() {
    let project = tempfile::tempdir().unwrap();
    write(
        project.path().join("margatroid-workspace.yaml"),
        r#"schema_version: 1
workspace:
  manager: coder
agents:
  coder:
    image: acme/coder:v1
    skills:
      - name: acme/outside
        path: ../outside
"#,
    );
    let error = Compiler::default()
        .compile(project.path().join("margatroid-workspace.yaml"))
        .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::InvalidPath);

    write(project.path().join("skills/acme/local/SKILL.md"), "test\n");
    write(
        project.path().join("margatroid-workspace.yaml"),
        &format!(
            r#"schema_version: 1
workspace:
  manager: coder
agents:
  coder:
    image: acme/coder:v1
    skills:
      - name: acme/local
        path: skills/acme/local
        expected_digest: sha256:{}
"#,
            "0".repeat(64)
        ),
    );
    let error = Compiler::default()
        .compile(project.path().join("margatroid-workspace.yaml"))
        .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::DigestMismatch);
}

#[test]
fn supports_yaml_merge_keys_and_ignores_x_extensions() {
    let project = tempfile::tempdir().unwrap();
    write(
        project.path().join("margatroid-workspace.yaml"),
        r#"schema_version: 1
x-agent: &agent
  image: acme/coder:v1
workspace:
  manager: coder
agents:
  coder:
    <<: *agent
    x-note: local
"#,
    );
    let output = Compiler::default()
        .compile(project.path().join("margatroid-workspace.yaml"))
        .unwrap();
    assert_eq!(
        output.bundle().spec.agents[0].image.as_str(),
        "acme/coder:v1"
    );
}

#[test]
fn enforces_yaml_alias_and_node_limits() {
    let project = tempfile::tempdir().unwrap();
    write(
        project.path().join("margatroid-workspace.yaml"),
        r#"schema_version: 1
x-agent: &agent
  image: acme/coder:v1
workspace:
  manager: coder
agents:
  coder: *agent
"#,
    );
    let error = Compiler::new(
        CompileOptions::default().with_limits(ProjectLimits::default().with_max_yaml_aliases(0)),
    )
    .compile(project.path().join("margatroid-workspace.yaml"))
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::InvalidYaml);

    let error = Compiler::new(
        CompileOptions::default().with_limits(ProjectLimits::default().with_max_yaml_nodes(1)),
    )
    .compile(project.path().join("margatroid-workspace.yaml"))
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::InvalidYaml);
}

#[test]
fn rejects_multiple_documents_and_explicit_yaml_tags() {
    for content in [
        "schema_version: 1\n---\nschema_version: 1\n",
        "schema_version: 1\nworkspace: !custom {manager: coder}\nagents: {}\n",
    ] {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("margatroid-workspace.yaml");
        write(&path, content);
        let error = Compiler::default().compile(path).unwrap_err();
        assert_eq!(error.diagnostics()[0].code, DiagnosticCode::InvalidYaml);
        assert!(error.diagnostics()[0].location.is_some());
    }
}

#[test]
fn normalizes_line_endings_and_uses_the_protocol_package_format() {
    fn compile_with_content(content: &str) -> (String, ResourcePackage) {
        let project = project();
        write(
            project
                .path()
                .join(".margatroid/skills/acme/reviewer/SKILL.md"),
            content,
        );
        let output = Compiler::default()
            .compile(project.path().join("margatroid-workspace.yaml"))
            .unwrap();
        let entry = output
            .bundle()
            .manifest
            .entries
            .iter()
            .find(|entry| entry.logical_name == "acme/reviewer")
            .unwrap();
        let bundled = output
            .bundle()
            .resources
            .iter()
            .find(|resource| resource.digest == entry.digest)
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&bundled.content_base64)
            .unwrap();
        (
            entry.digest.as_str().to_owned(),
            serde_json::from_slice(&bytes).unwrap(),
        )
    }

    let (lf_digest, package) = compile_with_content("line one\nline two\n");
    let (crlf_digest, _) = compile_with_content("line one\r\nline two\r\n");
    assert_eq!(lf_digest, crlf_digest);
    assert_eq!(package.format_version, 1);
    assert_eq!(package.files[0].path, "SKILL.md");
}

#[test]
fn enforces_compose_resource_and_package_entry_limits_before_completion() {
    let project = project();
    let compose = project.path().join("margatroid-workspace.yaml");
    let error = Compiler::new(
        CompileOptions::default().with_limits(ProjectLimits::default().with_max_compose_bytes(8)),
    )
    .compile(&compose)
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::ComposeTooLarge);

    write(
        project
            .path()
            .join(".margatroid/skills/acme/reviewer/extra.txt"),
        "extra",
    );
    let error = Compiler::new(
        CompileOptions::default()
            .with_limits(ProjectLimits::default().with_max_files_per_resource(1)),
    )
    .compile(&compose)
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::TooManyFiles);

    let error = Compiler::new(
        CompileOptions::default().with_limits(ProjectLimits::default().with_max_resource_bytes(4)),
    )
    .compile(compose)
    .unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code,
        DiagnosticCode::ResourceTooLarge
    );
}

#[test]
fn diagnostics_include_safe_file_and_field_locations() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("margatroid-workspace.yaml");
    write(
        &path,
        r#"schema_version: 2
workspace:
  manager: missing
agents:
  coder:
    image: acme/coder:v1
"#,
    );
    let error = Compiler::default().compile(&path).unwrap_err();
    let rendered = error.to_string();
    assert!(!rendered.contains(project.path().to_str().unwrap()));
    assert!(rendered.contains("margatroid-workspace.yaml"));
    assert!(rendered.contains("schema_version"));
    assert!(rendered.contains("workspace.manager"));
    assert_eq!(error.diagnostics().len(), 2);
}

#[test]
fn normalized_debug_output_does_not_contain_resource_bodies() {
    let project = project();
    let output = Compiler::default()
        .compile(project.path().join("margatroid-workspace.yaml"))
        .unwrap();
    let debug = format!("{:?}", output.normalized());
    assert!(!debug.contains("content_base64"));
    assert!(!debug.contains("cHJvamVjdCByZXZpZXdlcgo="));
}

#[test]
fn accepts_expected_digest_and_installed_resource_ids() {
    let project = project();
    let compose = project.path().join("margatroid-workspace.yaml");
    let first = Compiler::default().compile(&compose).unwrap();
    let digest = first
        .bundle()
        .manifest
        .entries
        .iter()
        .find(|entry| entry.logical_name == "acme/reviewer")
        .unwrap()
        .digest
        .as_str()
        .to_owned();
    write(
        &compose,
        &format!(
            r#"schema_version: 1
workspace:
  manager: coder
agents:
  coder:
    image: acme/coder:v1
    skills:
      - name: acme/reviewer
        path: .margatroid/skills/acme/reviewer
        expected_digest: {digest}
    workflows:
      - installed: workflow-1
"#,
        ),
    );
    let output = Compiler::default().compile(compose).unwrap();
    assert_eq!(output.bundle().manifest.entries.len(), 1);
    assert!(matches!(
        &output.bundle().spec.agents[0].workflows[0],
        margatroid_protocol::ResourceReference::Installed { id } if id.as_str() == "workflow-1"
    ));
}

#[test]
fn rejects_duplicate_logical_names_resolving_to_different_packages() {
    let project = tempfile::tempdir().unwrap();
    write(project.path().join("skills/first/SKILL.md"), "first\n");
    write(project.path().join("skills/second/SKILL.md"), "second\n");
    let compose = project.path().join("margatroid-workspace.yaml");
    write(
        &compose,
        r#"schema_version: 1
workspace:
  manager: coder
agents:
  coder:
    image: acme/coder:v1
    skills:
      - name: acme/shared
        path: skills/first
      - name: acme/shared
        path: skills/second
"#,
    );
    let error = Compiler::default().compile(compose).unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::DuplicateName);
}

#[test]
fn enforces_bundle_resource_count_and_yaml_depth_limits() {
    let project = project();
    let compose = project.path().join("margatroid-workspace.yaml");
    let error = Compiler::new(
        CompileOptions::default().with_limits(ProjectLimits::default().with_max_bundle_bytes(1)),
    )
    .compile(&compose)
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::BundleTooLarge);

    let error = Compiler::new(
        CompileOptions::default().with_limits(ProjectLimits::default().with_max_resources(1)),
    )
    .compile(&compose)
    .unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code,
        DiagnosticCode::TooManyResources
    );

    let error = Compiler::new(
        CompileOptions::default().with_limits(ProjectLimits::default().with_max_yaml_depth(1)),
    )
    .compile(compose)
    .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::InvalidYaml);
}

#[test]
fn rejects_absolute_resource_paths() {
    let project = tempfile::tempdir().unwrap();
    let compose = project.path().join("margatroid-workspace.yaml");
    write(
        &compose,
        &format!(
            r#"schema_version: 1
workspace:
  manager: coder
agents:
  coder:
    image: acme/coder:v1
    skills:
      - name: acme/outside
        path: {}
"#,
            project.path().display()
        ),
    );
    let error = Compiler::default().compile(compose).unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::InvalidPath);
}

#[cfg(unix)]
#[test]
fn rejects_resource_symlinks_that_escape_the_project() {
    use std::os::unix::fs::symlink;

    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(outside.path().join("SKILL.md"), "outside\n");
    fs::create_dir_all(project.path().join("skills/acme")).unwrap();
    symlink(outside.path(), project.path().join("skills/acme/outside")).unwrap();
    let compose = project.path().join("margatroid-workspace.yaml");
    write(
        &compose,
        r#"schema_version: 1
workspace:
  manager: coder
agents:
  coder:
    image: acme/coder:v1
    skills:
      - name: acme/outside
        path: skills/acme/outside
"#,
    );
    let error = Compiler::default().compile(compose).unwrap_err();
    assert_eq!(
        error.diagnostics()[0].code,
        DiagnosticCode::PathEscapesProject
    );
}

#[test]
fn rejects_non_utf8_resource_files() {
    let project = project();
    fs::write(
        project
            .path()
            .join(".margatroid/skills/acme/reviewer/binary.dat"),
        [0xff, 0xfe],
    )
    .unwrap();
    let error = Compiler::default()
        .compile(project.path().join("margatroid-workspace.yaml"))
        .unwrap_err();
    assert_eq!(error.diagnostics()[0].code, DiagnosticCode::InvalidResource);
}
