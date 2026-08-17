use std::fs;

use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use inference_plugin::{
    ContextCompactionInferenceRequest, ContextCompactionInferenceResponse, InferencePlugin,
    ReloadModelRoutesResult, WorldInferenceExt,
};
use tempfile::tempdir;

#[test]
fn documented_public_api_installs_and_reloads_routes() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("models.toml");
    fs::write(
        &path,
        r#"[[models]]
id = "test"
model = "test-model"
base_url = "https://example.test/v1"
api_key = "secret"
api_type = "openai"
"#,
    )
    .unwrap();

    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(InferencePlugin::default().with_config_path(&path));
    app.world().reload_model_routes("reload-1");
    app.tick();
    app.tick();

    let result = app
        .world()
        .event_reader::<ReloadModelRoutesResult>()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(result.id, "reload-1");
    assert_eq!(result.result.as_ref().unwrap().route_count, 1);
}

#[test]
fn context_compaction_inference_events_are_public() {
    fn assert_event<EventType: core_plugin::Event>() {}
    assert_event::<ContextCompactionInferenceRequest>();
    assert_event::<ContextCompactionInferenceResponse>();
}
