use app_runtime_plugin::AppRunExt;
use axum_plugin::AxumHandle;
use compose::{ApplicationOptions, build_app};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const CHILD_ROOT: &str = "MARGATROID_NEW_POST_STOP_TEST_ROOT";
const NAME: &str =
    "original_websocket_reads_registered_workspaces_and_no_agents_after_real_task_stop";
struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn runner() -> PathBuf {
    let path = std::env::var_os("MARGATROID_TEST_RUNNER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/margatroid")
        });
    assert!(
        path.is_file(),
        "build the runner with cargo build -p margatroid or set MARGATROID_TEST_RUNNER: {}",
        path.display()
    );
    path.canonicalize().unwrap()
}
fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
async fn send(socket: &mut Socket, kind: &str, id: &str, message: Value) {
    socket
        .send(Message::Text(
            json!({"type":kind,"id":id,"message":message})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}
async fn next(socket: &mut Socket, matches: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Message::Text(text) = socket
                .next()
                .await
                .expect("socket closed")
                .expect("read failed")
            {
                let value: Value = serde_json::from_str(&text).unwrap();
                if matches(&value) {
                    return value;
                }
            }
        }
    })
    .await
    .expect("event-driven daemon did not reply")
}
#[test]
fn original_websocket_reads_registered_workspaces_and_no_agents_after_real_task_stop() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let mut app = build_app(ApplicationOptions {
            data_root: root.clone(),
            runner_path: runner(),
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let address = loop {
            app.tick();
            if let Some(address) = app
                .world()
                .get_resource::<AxumHandle>()
                .and_then(AxumHandle::local_address)
            {
                break address;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        std::fs::write(root.join("server-address"), address.to_string()).unwrap();
        app.run();
        unreachable!();
    }
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "config.toml", "[server]\nbind='127.0.0.1:0'\n");
    write(
        root.path(),
        "models.toml",
        "[[models]]\nid='offline'\nmodel='unused'\nprovider='openai'\napi_type='chat_completions'\nbase_url='http://127.0.0.1:1/v1'\napi_key='test'\ncontext_window='128k'\n",
    );
    for dir in [
        "tools",
        "prompts",
        "sandboxes",
        "skills",
        "workspaces",
        "project",
    ] {
        std::fs::create_dir_all(root.path().join(dir)).unwrap();
    }
    let dir = root.path().join("tools/probe/driver/dev");
    write(
        &dir,
        "tool.toml",
        "schema_version=1\nname='driver'\ndescription='Create post-stop test Agent'\n",
    );
    write(
        &dir,
        "input.schema.json",
        r#"{"type":"object","properties":{"node_name":{"type":"string"}},"required":["node_name"],"additionalProperties":false}"#,
    );
    write(&dir, "setting.toml", "");
    write(&dir, "ui/index.html", "<html>Agent</html>");
    write(
        &dir,
        "main.lua",
        r#"
function configure()return {timeout_ms=0}end
function execute(args,context)
 local agent=margatroid.agent:create({node_name=args.node_name,project_root=context.project_root,config_path=context.resource_dir..'/setting.toml',ui_path=context.resource_dir..'/ui'})
 local ready=margatroid.mcl:execute(agent.address,'EFFECT ready')
 if ready.Error then error(ready.Error.message)end
 margatroid.task:return_result(agent.address)
 margatroid.task:receive()
end
"#,
    );
    let mut daemon = Daemon(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD_ROOT, root.path())
            .env("MARGATROID_TEST_RUNNER", runner())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let address = loop {
        if let Ok(address) = std::fs::read_to_string(root.path().join("server-address"))
            && let Ok(address) = address.parse::<std::net::SocketAddr>()
        {
            break address;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "daemon child exited"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
  let(mut socket,_)=tokio_tungstenite::connect_async(format!("ws://{address}/ws")).await.unwrap();
  send(&mut socket,"connection.register","register",json!({"client_type":"webui","name":"post-stop"})).await;assert_eq!(next(&mut socket,|v|v["id"]=="register").await["success"],true);
  send(&mut socket,"workspace.create","workspace",json!({"address":"/demo","alias":"Demo","project_root":root.path().join("project")})).await;let result=next(&mut socket,|v|v["id"]=="workspace").await;assert_eq!(result["success"],true,"{result}");
  let mut record_id=Value::Null;
  for cycle in 0..2{
   let start_id=format!("start-{cycle}");let message=if cycle==0{json!({"mode":"new","workspace":"/demo","tool_path":dir.join("tool.toml"),"arguments":{"node_name":"coder"}})}else{json!({"mode":"existing","workspace":"/demo","node_name":"coder"})};
   send(&mut socket,"agent.start",&start_id,message).await;let started=next(&mut socket,|v|v["id"]==start_id).await;assert_eq!(started["success"],true,"{started}");
   send(&mut socket,"agent.req","live",json!({})).await;let live=next(&mut socket,|v|v["id"]=="live").await;assert_eq!(live["agents"][0]["address"],"/demo/coder");assert_eq!(live["agents"][0]["status"],"running");
   let stop_id=format!("stop-{cycle}");send(&mut socket,"agent.stop",&stop_id,json!({"address":"/demo/coder"})).await;let stopped=next(&mut socket,|v|v["id"]==stop_id).await;assert_eq!(stopped["success"],true,"{stopped}");
   send(&mut socket,"agent.req","empty",json!({})).await;assert_eq!(next(&mut socket,|v|v["id"]=="empty").await["agents"],json!([]));
   send(&mut socket,"workspace.req","registered",json!({})).await;let workspace=next(&mut socket,|v|v["id"]=="registered").await;let row=&workspace["workspaces"][0];assert_eq!(row["address"],"/demo");assert_eq!(row["status"],"running");assert_eq!(row["agents"][0]["node_name"],"coder");assert_eq!(row["agents"][0]["category"],"normal");if cycle==0{record_id=row["agents"][0]["id"].clone();}else{assert_eq!(row["agents"][0]["id"],record_id);}
  }
  send(&mut socket,"agent.archive","archive",json!({"workspace":"/demo","node_name":"coder"})).await;assert_eq!(next(&mut socket,|v|v["id"]=="archive").await["success"],true);
  send(&mut socket,"workspace.req","archived",json!({})).await;let workspace=next(&mut socket,|v|v["id"]=="archived").await;assert_eq!(workspace["workspaces"][0]["agents"][0]["category"],"archived");
  send(&mut socket,"workspace.delete","delete",json!({"address":"/demo"})).await;assert_eq!(next(&mut socket,|v|v["id"]=="delete").await["success"],true);
  send(&mut socket,"workspace.req","after-delete",json!({})).await;assert_eq!(next(&mut socket,|v|v["id"]=="after-delete").await["workspaces"],json!([]));socket.close(None).await.unwrap();
 });
    assert!(!root.path().join("workspaces/demo").exists());
    assert!(root.path().join("project").is_dir());
}
