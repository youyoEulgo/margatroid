use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use margatroid_types::ResourceName;
use skill_loader_plugin::{
    LoadSkill, LoadSkillResult, SkillLoadErrorKind, SkillLoaderPlugin, SkillSourceRoots,
    SkillVisibility,
};

fn unique_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "margatroid-skill-loader-public-api-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn documented_public_api_composes_from_an_external_crate() {
    let directory = unique_directory();
    let home = directory.join("home");
    let project = directory.join("project");
    let image = directory.join("image");
    let name = ResourceName::new("local/review").unwrap();

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(SkillLoaderPlugin::open(&home).unwrap());
    let agent = app.world_mut().spawn();
    app.world_mut()
        .insert_component(agent, SkillVisibility::new().with([name.clone()]));
    app.world_mut()
        .insert_component(agent, SkillSourceRoots::new(project, image).unwrap());
    app.world().send_event(LoadSkill {
        id: "public-api".into(),
        agent,
        name,
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<LoadSkillResult>()
            .into_iter()
            .next()
        {
            assert_eq!(event.id, "public-api");
            let Err(error) = &event.result else {
                panic!("missing public API fixture must not load successfully");
            };
            assert_eq!(error.kind(), SkillLoadErrorKind::NotFound);
            break;
        }
        assert!(Instant::now() < deadline, "skill load timed out");
        std::thread::yield_now();
    }

    let _ = fs::remove_dir_all(directory);
}
