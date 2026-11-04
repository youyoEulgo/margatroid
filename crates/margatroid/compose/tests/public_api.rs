use compose::{ApplicationOptions, build_app};
use std::fs;

#[test]
fn assembly_reports_domain_and_path_before_starting_services() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("workspaces")).unwrap();
    let options = ApplicationOptions {
        data_root: root.path().to_owned(),
        runner_path: std::env::current_exe().unwrap(),
    };
    let error = build_app(options.clone())
        .err()
        .expect("missing config should fail");
    assert_eq!(error.domain, "config");
    assert_eq!(error.path, root.path().join("config.toml"));
    fs::write(root.path().join("config.toml"),"[server]\nbind='127.0.0.1:0'\n[outbound]\nlogs=['broadcast']\nbackend_state=['broadcast']\n").unwrap();
    let error = build_app(options)
        .err()
        .expect("missing model routes should fail");
    assert_eq!(error.domain, "inference");
    assert_eq!(error.path, root.path().join("models.toml"));
    assert!(!root.path().join("workspaces/agents").exists());
}

#[test]
fn domain_cores_are_independent_and_plugins_only_depend_on_cores() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    for domain in [
        "agent",
        "agent_driver",
        "client",
        "config",
        "workspace",
        "lua_runtime",
        "storage",
        "mcl",
        "tool",
        "inference",
        "tool_resource",
        "prompt_resource",
        "sandbox_resource",
        "dto",
    ] {
        for layer in ["core", "plugin"] {
            let manifest = root.join(domain).join(layer).join("Cargo.toml");
            if !manifest.is_file() {
                continue;
            }
            let document: toml::Value = fs::read_to_string(&manifest).unwrap().parse().unwrap();
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                let Some(dependencies) = document.get(section).and_then(toml::Value::as_table)
                else {
                    continue;
                };
                for (name, definition) in dependencies {
                    let Some(path) = definition.get("path").and_then(toml::Value::as_str) else {
                        continue;
                    };
                    let target = manifest
                        .parent()
                        .unwrap()
                        .join(path)
                        .canonicalize()
                        .unwrap();
                    if !target.starts_with(root) {
                        continue;
                    }
                    assert_ne!(layer, "core", "{domain}/core depends on domain {name}");
                    assert!(
                        target.ends_with("core")
                            || target.ends_with("address")
                            || target.ends_with("protocol")
                            || (domain == "dto"
                                && name == "axum_plugin"
                                && target.ends_with("axum_plugin")),
                        "{domain}/plugin has a domain plugin dependency: {name}"
                    );
                }
            }
        }
    }
    for package in ["protocol_next", "protocol"] {
        let manifest = root.join(package).join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let document: toml::Value = fs::read_to_string(manifest).unwrap().parse().unwrap();
        if package == "protocol" && root.join("protocol_next").is_dir() {
            continue;
        }
        for (_, dep) in document["dependencies"].as_table().unwrap() {
            assert!(
                dep.get("path").is_none(),
                "network protocol must have only general library dependencies"
            );
        }
    }
}
