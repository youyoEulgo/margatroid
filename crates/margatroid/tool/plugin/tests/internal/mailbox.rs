#![cfg(target_os = "linux")]
use super::*;

#[test]
fn driver_wait_host_request_does_not_block_workspace_task_mailbox() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("default.toml"), "[custom]\nvalue=1\n").unwrap();
    let mut app = app();
    let workspace = mount_workspace(&mut app, root.path());
    app.add_system(RuntimePlugin::UPDATE, agent_fixture);
    let driver = tool(
        root.path(),
        "driver-mailbox",
        r#"
function execute(arguments)
 local agent=margatroid.agent:create({node_name='mailbox',project_root=arguments.project_root,config_path=arguments.config_path,ui_path=arguments.ui_path})
 margatroid.task:return_result('ready')
 local which,value=margatroid:wait_any({wake=function() margatroid:wait(agent.address) end,inbox=function() return margatroid.task:receive() end})
 assert(which=='inbox' and value=='mailbox-message')
 margatroid.files:write(arguments.marker,'delivered')
end"#,
    );
    let marker = root.path().join("mailbox-delivered");
    let reply = execute(
        &mut app,
        workspace,
        driver,
        json!({"project_root":root.path(),"config_path":root.path().join("default.toml"),"ui_path":root.path().join("ui"),"marker":marker}),
    );
    assert_eq!(wait(&mut app, reply).unwrap().content, "ready");
    let tasks = app
        .world()
        .get_component::<ToolTasks>(workspace)
        .unwrap()
        .clone();
    let (id, _) = tasks.list().into_iter().next().unwrap();
    tasks.send(&id, json!("mailbox-message")).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        app.tick();
        assert!(
            Instant::now() < until,
            "Host(wait) blocked the runner mailbox pump"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "delivered");
    drain(&mut app, workspace);
}
