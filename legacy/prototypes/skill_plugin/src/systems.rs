use std::path::{Path, PathBuf};

use core_plugin::{EventReader, World};
use serde::Deserialize;

use crate::events::{
    SkillLoadFailed, SkillLoadRequested, SkillLoaded, SkillScanFailed, SkillScanRequested,
    SkillScanned, SkillUnloadRequested, SkillUnloaded,
};
use crate::resource::{LoadedSkills, SkillDescriptor, SkillKind, SkillRegistry};

#[derive(Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: Option<String>,
    allowed_tools: Option<Vec<String>>,
    preload: Option<bool>,
}

pub(crate) fn scan_requested_skills(
    world: &mut World,
    reader: &mut EventReader<SkillScanRequested>,
) {
    for request in world.read_events(reader) {
        match scan_skill_root(&request.root) {
            Ok(skills) => {
                let registry = world
                    .resource::<SkillRegistry>()
                    .expect("SkillRegistry should be registered by SkillPlugin");
                for skill in &skills {
                    if let Err(error) = registry.replace(skill.clone()) {
                        world.send_event(SkillScanFailed {
                            root: request.root.clone(),
                            message: error.to_string(),
                        });
                    }
                }
                world.send_event(SkillScanned {
                    root: request.root,
                    skills,
                });
            }
            Err(message) => world.send_event(SkillScanFailed {
                root: request.root,
                message,
            }),
        }
    }
}

pub(crate) fn load_requested_skills(
    world: &mut World,
    reader: &mut EventReader<SkillLoadRequested>,
) {
    for request in world.read_events(reader) {
        let Some(skill) = world
            .resource::<SkillRegistry>()
            .expect("SkillRegistry should be registered by SkillPlugin")
            .get(&request.name)
        else {
            world.send_event(SkillLoadFailed {
                name: request.name,
                message: "skill is not registered".into(),
            });
            continue;
        };

        world
            .resource::<LoadedSkills>()
            .expect("LoadedSkills should be registered by SkillPlugin")
            .load(skill.clone());
        world.send_event(SkillLoaded { skill });
    }
}

pub(crate) fn unload_requested_skills(
    world: &mut World,
    reader: &mut EventReader<SkillUnloadRequested>,
) {
    for request in world.read_events(reader) {
        world
            .resource::<LoadedSkills>()
            .expect("LoadedSkills should be registered by SkillPlugin")
            .unload(&request.name);
        world.send_event(SkillUnloaded { name: request.name });
    }
}

fn scan_skill_root(root: &Path) -> Result<Vec<SkillDescriptor>, String> {
    let mut skills = Vec::new();
    collect_skill_files(root, &mut skills)?;
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

fn collect_skill_files(path: &Path, skills: &mut Vec<SkillDescriptor>) -> Result<(), String> {
    if path.is_file() {
        if path.extension().is_some_and(|ext| ext == "md") {
            skills.push(parse_skill_file(path)?);
        }
        return Ok(());
    }

    for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_skill_files(&path, skills)?;
        } else if path.extension().is_some_and(|ext| ext == "md") {
            skills.push(parse_skill_file(&path)?);
        }
    }
    Ok(())
}

fn parse_skill_file(path: &Path) -> Result<SkillDescriptor, String> {
    let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let (frontmatter, body) = split_frontmatter(&content)?;
    let metadata: SkillFrontmatter =
        toml::from_str(frontmatter).map_err(|error| error.to_string())?;
    let kind = if body.contains("[[steps]]") {
        SkillKind::Workflow
    } else {
        SkillKind::Member
    };
    Ok(SkillDescriptor {
        name: metadata.name,
        description: metadata.description.unwrap_or_default(),
        allowed_tools: metadata.allowed_tools.unwrap_or_default(),
        preload: metadata.preload.unwrap_or(false),
        kind,
        path: PathBuf::from(path),
        body: body.trim().to_string(),
    })
}

fn split_frontmatter(content: &str) -> Result<(&str, &str), String> {
    let Some(rest) = content.strip_prefix("+++\n") else {
        return Err("skill file must start with TOML frontmatter delimiter `+++`".into());
    };
    let Some(end) = rest.find("\n+++") else {
        return Err("skill file is missing closing TOML frontmatter delimiter `+++`".into());
    };
    let frontmatter = &rest[..end];
    let body = rest[end + "\n+++".len()..].trim_start_matches(['\r', '\n']);
    Ok((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use core_plugin::{App, Stage, World};

    use crate::{
        LoadedSkills, SkillKind, SkillLoadRequested, SkillLoaded, SkillPlugin, SkillScanRequested,
        SkillScanned,
    };

    #[test]
    fn plugin_scans_and_loads_skills() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("coder.md"),
            r#"+++
name = "coder"
description = "Writes Rust"
allowed_tools = ["bash"]
preload = true
+++

# coder
"#,
        )
        .unwrap();

        let mut app = App::new();
        app.add_plugins(SkillPlugin::new());

        let scanned = Arc::new(Mutex::new(Vec::new()));
        let system_scanned = scanned.clone();
        let mut scan_reader = app.event_reader::<SkillScanned>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                system_scanned
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut scan_reader));
            }],
        );

        app.world().send_event(SkillScanRequested::new(dir.path()));
        app.tick();
        assert_eq!(scanned.lock().unwrap()[0].skills.len(), 1);
        assert_eq!(scanned.lock().unwrap()[0].skills[0].kind, SkillKind::Member);

        let loaded = Arc::new(Mutex::new(Vec::new()));
        let system_loaded = loaded.clone();
        let mut load_reader = app.event_reader::<SkillLoaded>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                system_loaded
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut load_reader));
            }],
        );

        app.world().send_event(SkillLoadRequested::new("coder"));
        app.tick();

        assert_eq!(loaded.lock().unwrap().len(), 1);
        assert!(app
            .world()
            .resource::<LoadedSkills>()
            .unwrap()
            .get("coder")
            .is_some());
    }
}
