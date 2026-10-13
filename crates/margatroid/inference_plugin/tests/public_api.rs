use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use inference_plugin::{
    InferencePlugin, InferenceRequest, ReloadModelRoutes, ReloadModelRoutesResult,
};
use margatroid_types::message_types::AgentMessage;
use std::fs;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn public_api_installs_reloads_and_replies_to_missing_agent() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("models.toml");
    fs::write(&path, "[[models]]\nid='test'\nmodel='test-model'\nbase_url='https://example.test/v1'\napi_key='secret'\napi_type='openai'\n").unwrap();
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::PRE_UPDATE)
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(InferencePlugin::default().with_config_path(&path));
    let agent = app.world_mut().spawn();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    app.world().send_event(InferenceRequest {
        id: "request".into(),
        agent,
        messages: vec![AgentMessage::User {
            content: "hello".into(),
        }],
        tools: vec![],
        turn: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        reply: std::sync::Arc::new(std::sync::Mutex::new(Some(tx))),
    });
    app.tick();
    assert_eq!(
        rx.try_recv().unwrap().unwrap_err().kind,
        inference_plugin::InferenceErrorKind::AgentMissing
    );
    app.world().send_event(ReloadModelRoutes {
        id: "reload-1".into(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if let Some(result) = app
            .world()
            .event_reader::<ReloadModelRoutesResult>()
            .into_iter()
            .find(|r| r.id == "reload-1")
        {
            assert_eq!(result.result, Ok(1));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    fs::write(&path, "not valid toml").unwrap();
    app.world()
        .send_event(ReloadModelRoutes { id: "bad".into() });
    app.world()
        .send_event(ReloadModelRoutes { id: "busy".into() });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_busy = false;
    let mut saw_bad = false;
    while !saw_busy || !saw_bad {
        app.tick();
        for result in app.world().event_reader::<ReloadModelRoutesResult>() {
            if result.id == "busy" {
                assert_eq!(
                    result.result.as_ref().unwrap_err().kind,
                    inference_plugin::InferenceErrorKind::ConfigBusy
                );
                saw_busy = true;
            } else if result.id == "bad" {
                assert!(result.result.is_err());
                saw_bad = true;
            }
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        app.world()
            .get_resource::<inference_plugin::InferenceConfig>()
            .unwrap()
            .model_count(),
        1
    );
}
