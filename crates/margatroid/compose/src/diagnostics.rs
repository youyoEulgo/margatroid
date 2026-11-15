use core_plugin::App;
use storage_core::{
    AgentLogLevel, AgentLogRecord, AgentLogSource, RuntimeLogInput, RuntimeLogStore,
};

pub(crate) fn install(app: &mut App, store: RuntimeLogStore) {
    let tracing = app
        .world()
        .get_resource::<log_plugin::TracingStream>()
        .expect("persistent diagnostics require tracing source")
        .clone();
    let writer = store.clone();
    let guard=tracing.subscribe_persistent(move|event|{
        let level=match event.level.as_str(){"TRACE"|"DEBUG"=>AgentLogLevel::Debug,"INFO"=>AgentLogLevel::Info,"WARN"=>AgentLogLevel::Warn,_=>AgentLogLevel::Error};
        let error=event.fields.iter().find(|field|matches!(field.name.as_str(),"error"|"reason"|"raw_error")).map(|field|field.value.clone());
        let traceback=if level==AgentLogLevel::Error{Some(std::backtrace::Backtrace::force_capture().to_string())}else{None};
        let record=RuntimeLogInput{report_id:None,reporter:"backend".into(),target:event.target.clone(),agent:None,record:AgentLogRecord{created_at_ms:Some(event.timestamp_millis),level,source:AgentLogSource::Host,stage:"runtime".into(),message:event.message.clone(),details_json:Some(serde_json::json!({"fields":event.fields,"spans":event.spans,"tracing_level":event.level,"file":event.file,"line":event.line,"module":event.module})),context_json:Some(serde_json::json!({"process_id":std::process::id()})),origin:event.file.as_ref().map(|file|event.line.map_or_else(||file.clone(),|line|format!("{file}:{line}"))),raw_error:error,traceback}};
        if let Err(error)=writer.append(record){eprintln!("runtime diagnostic could not be persisted: {error}; original: {}",event.message);}
    });
    app.world_mut().insert_resource(store);
    app.world_mut().insert_resource(guard);
}

pub(crate) fn startup_failure(options: &crate::ApplicationOptions, error: &crate::ComposeError) {
    if !options.data_root.is_dir() {
        return;
    }
    let store = storage_core::open_runtime_logs(
        &options.data_root.join("runtime.sqlite"),
        storage_core::RuntimeLogProbe::new(|_| {}),
    );
    let input = RuntimeLogInput {
        report_id: None,
        reporter: "backend".into(),
        target: "startup".into(),
        agent: None,
        record: AgentLogRecord {
            created_at_ms: None,
            level: AgentLogLevel::Error,
            source: AgentLogSource::Host,
            stage: "startup".into(),
            message: error.to_string(),
            details_json: Some(serde_json::json!({"domain":error.domain,"path":error.path})),
            context_json: Some(serde_json::json!({"process_id":std::process::id()})),
            origin: Some("compose::build_app".into()),
            raw_error: Some(format!("{error:?}")),
            traceback: Some(std::backtrace::Backtrace::force_capture().to_string()),
        },
    };
    if let Err(failure) = store.and_then(|store| store.append(input)) {
        eprintln!("startup diagnostic persistence failed: {failure}; original: {error}");
    }
}
