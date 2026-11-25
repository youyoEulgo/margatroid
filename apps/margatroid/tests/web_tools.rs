use mlua::{Lua, LuaSerdeExt, Value};
use serde_json::{json, Value as Json};
use std::sync::{Arc, Mutex};

fn run(name: &str, args: Json, key: &str, replies: Vec<(u16, String)>) -> (Json, Vec<Json>) {
    let lua = Lua::new();
    let callbacks = lua.create_table().unwrap();
    callbacks
        .set(
            "data_encode",
            lua.create_function(|lua, (format, value): (String, Value)| {
                assert_eq!(format, "json");
                let value: Json = lua.from_value(value)?;
                Ok(value.to_string())
            })
            .unwrap(),
        )
        .unwrap();
    callbacks
        .set(
            "data_decode",
            lua.create_function(|lua, (format, value): (String, String)| {
                assert_eq!(format, "json");
                let value: Json = serde_json::from_str(&value).map_err(mlua::Error::external)?;
                lua.to_value(&value)
            })
            .unwrap(),
        )
        .unwrap();
    callbacks.set("data_null", lua.null()).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let requests = calls.clone();
    let replies = Arc::new(Mutex::new(replies.into_iter()));
    callbacks
        .set(
            "http",
            lua.create_function(move |lua, (_, options): (Value, Value)| {
                let value: Json = lua.from_value(options)?;
                requests.lock().unwrap().push(value);
                let (status, body) = replies
                    .lock()
                    .unwrap()
                    .next()
                    .expect("unexpected HTTP request");
                lua.to_value(&json!({"status":status,"body":body}))
            })
            .unwrap(),
        )
        .unwrap();
    lua.globals().set("callbacks", callbacks).unwrap();
    lua.globals().set("key", key).unwrap();
    lua.load(
        r#"
        config_reads=0
        margatroid={data={},mcl={},http={},task={}}
        function margatroid.data:encode(format,value)return callbacks.data_encode(format,value)end
        function margatroid.data:decode(format,value)return callbacks.data_decode(format,value)end
        margatroid.data.null=callbacks.data_null
        function margatroid.mcl:execute(address,command)
            assert(address=='/demo/caller' and command=='CONFIG GET')
            config_reads=config_reads+1
            return {Config={web={deepseek={api_key=key}}}}
        end
        margatroid.http.request=callbacks.http
        function margatroid.task:return_result(value)result=value end
        function margatroid.task:finish(id)assert(id=='test-task');finished=true end
    "#,
    )
    .exec()
    .unwrap();
    let source = std::fs::read_to_string(format!(
        "{}/../../resources/tools/margatroid/{name}/dev/main.lua",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    lua.load(source).exec().unwrap();
    lua.globals()
        .get::<mlua::Function>("execute")
        .unwrap()
        .call::<()>((
            lua.to_value(&args).unwrap(),
            lua.to_value(&json!({"agent_info":{"address":"/demo/caller"},"task_id":"test-task"}))
                .unwrap(),
        ))
        .unwrap();
    assert!(lua.globals().get::<bool>("finished").unwrap());
    let result = lua
        .from_value(lua.globals().get::<Value>("result").unwrap())
        .unwrap();
    let calls = calls.lock().unwrap().clone();
    (result, calls)
}

fn source(url: &str) -> Json {
    json!({"url":url,"title":url,"snippet":"short","summary":"long","date":null})
}

#[test]
fn search_deduplicates_queries_merges_sources_and_uses_private_agent_key() {
    let (result, calls) = run(
        "web-search",
        json!({"queries":["one","two","one"]}),
        "mock-secret",
        vec![
            (
                200,
                json!({"results":[source("https://a"),source("https://shared")]}).to_string(),
            ),
            (
                200,
                json!({"results":[source("https://b"),source("https://shared")]}).to_string(),
            ),
        ],
    );
    assert_eq!(result["ok"], true);
    assert_eq!(calls.len(), 2);
    for call in &calls {
        assert_eq!(call["url"], "https://api.deepseek.com/v1/tools/search");
        assert_eq!(call["method"], "POST");
        assert_eq!(call["headers"][0]["value"], "Bearer mock-secret");
        let body: Json = serde_json::from_str(call["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["need_content"], false);
        assert_eq!(body["count"], 10);
    }
    let text = result["content"].as_str().unwrap();
    assert!(text.find("[https://a]").unwrap() < text.find("[https://b]").unwrap());
    assert_eq!(text.matches("[https://shared]").count(), 1);
    assert!(!text.contains("mock-secret"));
}

#[test]
fn missing_key_never_dispatches_http_and_error_status_never_exposes_body() {
    for name in ["web-search", "web-fetch"] {
        let args = if name == "web-search" {
            json!({"queries":["query"]})
        } else {
            json!({"url":"https://example.com/page"})
        };
        let (result, calls) = run(name, args.clone(), "   ", vec![]);
        assert_eq!(result["ok"], false);
        assert!(calls.is_empty());
        for status in [401, 402, 429] {
            let (result, calls) = run(
                name,
                args.clone(),
                "mock-secret",
                vec![(status, "raw-private-error mock-secret".into())],
            );
            assert_eq!(calls.len(), 1);
            assert_eq!(result["ok"], false);
            assert!(!result.to_string().contains("raw-private-error"));
            assert!(!result.to_string().contains("mock-secret"));
        }
    }
}

#[test]
fn fetch_preserves_markdown_and_posts_url_to_fixed_endpoint() {
    let (result, calls) = run(
        "web-fetch",
        json!({"url":"https://example.com/page"}),
        "mock-secret",
        vec![(
            200,
            json!({"title":"Title","content":"# Heading\n\nBody"}).to_string(),
        )],
    );
    assert_eq!(result["ok"], true);
    assert!(result["content"]
        .as_str()
        .unwrap()
        .contains("# Heading\n\nBody"));
    assert_eq!(calls[0]["url"], "https://api.deepseek.com/v1/tools/fetch");
    let body: Json = serde_json::from_str(calls[0]["body"].as_str().unwrap()).unwrap();
    assert_eq!(body, json!({"url":"https://example.com/page"}));
}

#[test]
fn malformed_responses_fail_without_leaking_raw_input() {
    for (name, args) in [
        ("web-search", json!({"queries":["query"]})),
        ("web-fetch", json!({"url":"https://example.com"})),
    ] {
        for response in ["not-json-private", "{}"] {
            let (result, _) = run(
                name,
                args.clone(),
                "mock-secret",
                vec![(200, response.into())],
            );
            assert_eq!(result["ok"], false);
            assert!(!result.to_string().contains("not-json-private"));
        }
    }
}
