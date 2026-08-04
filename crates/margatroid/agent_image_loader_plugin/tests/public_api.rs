use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_image_loader_plugin::{
    AgentImageDefaultVisibility, AgentImageIdentity, AgentImageLoaderPlugin, AgentImageModelConfig,
    AgentImageSoul, LoadAgentImage, LoadAgentImageResult,
};
use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use margatroid_types::AgentImageReference;

fn unique_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "margatroid-agent-image-loader-public-api-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn write_image(library: &Path) {
    let image = library.join("local/coder/latest");
    fs::create_dir_all(&image).unwrap();
    fs::write(
        image.join("agent.toml"),
        "schema_version = 1\n[inference]\nmodel = \"test-model\"\n",
    )
    .unwrap();
    fs::write(image.join("SOUL.md"), "Public API soul.\n").unwrap();
}

#[test]
fn documented_public_api_composes_from_an_external_crate() {
    let library = unique_directory();
    write_image(&library);
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(AgentImageLoaderPlugin::open(&library).unwrap());
    let reference = AgentImageReference::new("local/coder").unwrap();
    app.world().send_event(LoadAgentImage {
        id: "public-api".into(),
        reference: reference.clone(),
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    let image = loop {
        app.tick();
        if let Some(event) = app
            .world()
            .event_reader::<LoadAgentImageResult>()
            .into_iter()
            .find(|event| event.id == "public-api")
        {
            let Ok(image) = event.result else {
                panic!("valid public API fixture must load successfully");
            };
            break image;
        }
        assert!(Instant::now() < deadline, "agent image load timed out");
        std::thread::yield_now();
    };

    assert_eq!(
        app.world()
            .get_component::<AgentImageIdentity>(image)
            .unwrap()
            .reference(),
        &reference
    );
    assert_eq!(
        app.world()
            .get_component::<AgentImageSoul>(image)
            .unwrap()
            .as_str(),
        "Public API soul.\n"
    );
    assert_eq!(
        app.world()
            .get_component::<AgentImageModelConfig>(image)
            .unwrap()
            .model(),
        "test-model"
    );
    assert_eq!(
        app.world()
            .get_component::<AgentImageDefaultVisibility>(image)
            .unwrap()
            .skills()
            .count(),
        0
    );
    let _ = fs::remove_dir_all(library);
}
