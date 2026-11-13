use std::time::Duration;
use tool_core::{ToolConfiguration, ToolExecutionLimits, ToolTaskHandle};

#[test]
fn configuration_has_one_lifetime_and_zero_means_unlimited() {
    let finite = ToolConfiguration {
        timeout_ms: 86_400_000,
    };
    finite.validate().unwrap();
    assert_eq!(finite.timeout(), Some(Duration::from_secs(86_400)));
    let unlimited = ToolConfiguration { timeout_ms: 0 };
    unlimited.validate().unwrap();
    assert_eq!(unlimited.timeout(), None);
    let limits = serde_json::to_value(ToolExecutionLimits::default()).unwrap();
    assert!(
        limits
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.contains("timeout"))
    );
}

#[test]
fn serialized_configuration_rejects_missing_unknown_and_noninteger_fields() {
    for configuration in [
        r#"{}"#,
        r#"{"timeout_ms":10,"host_call_timeout_ms":10}"#,
        r#"{"timeout_ms":10,"host_call_timeout_ms":10,"cleanup_timeout_ms":10}"#,
        r#"{"timeout_ms":10,"other":1}"#,
        r#"{"timeout_ms":1.5}"#,
        r#"{"timeout_ms":-1}"#,
        r#"{"timeout_ms":null}"#,
    ] {
        assert!(serde_json::from_str::<ToolConfiguration>(configuration).is_err());
    }
    for timeout_ms in [0, 10] {
        let configuration: ToolConfiguration =
            serde_json::from_value(serde_json::json!({"timeout_ms":timeout_ms})).unwrap();
        configuration.validate().unwrap();
    }
}

#[tokio::test]
async fn task_lifetime_is_configured_once_and_does_not_complete_before_cleanup() {
    let (task, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        tokio::sync::oneshot::channel().0,
        test_tasks().update_context(),
    )
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), task.lifetime_expired())
            .await
            .is_err()
    );
    task.configure_lifetime(20).unwrap();
    assert!(task.configure_lifetime(0).is_err());
    tokio::time::timeout(Duration::from_secs(1), task.lifetime_expired())
        .await
        .unwrap();
    task.cancel();
    assert!(task.cancel_token().is_cancelled());
    assert!(!task.is_cleanup_complete());
    test_tasks().complete_cleanup("task", &task, None);
    assert!(task.is_cleanup_complete());
}

#[tokio::test]
async fn unlimited_task_has_no_lifetime_timer_and_can_finish() {
    let (task, _input) = ToolTaskHandle::new(
        &ToolExecutionLimits::default(),
        "call".into(),
        "tool".into(),
        tokio::sync::oneshot::channel().0,
        test_tasks().update_context(),
    )
    .unwrap();
    task.configure_lifetime(0).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(60), task.lifetime_expired())
            .await
            .is_err()
    );
    assert!(!task.is_cleanup_complete());
    let supervised = task.clone();
    task.attach_task(tokio::spawn(async move {
        supervised.cancel_token().cancelled().await;
        tokio::task::yield_now().await;
        test_tasks().complete_cleanup("task", &supervised, None);
    }))
    .unwrap();
    task.finish().await.unwrap();
    assert!(task.is_cleanup_complete());
}

fn test_binding() -> tool_core::TaskSyncBinding {
    tool_core::TaskSyncBinding::new(
        client_core::Clients::default(),
        std::sync::Arc::from([]),
        |snapshot| {
            serde_json::to_string(&snapshot)
                .map(|text| std::sync::Arc::new(client_core::ClientFrame::Text(text)))
                .map_err(|_| client_core::ClientPrepareError::EncodeFailed)
        },
    )
}
fn test_tasks() -> tool_core::ToolTasks {
    let mut app = core_plugin::App::new();
    let owner = app.world_mut().spawn();
    tool_core::ToolTasks::new(owner, "agent:test/unit:latest".into(), test_binding())
}
