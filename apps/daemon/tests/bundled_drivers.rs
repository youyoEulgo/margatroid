use std::path::Path;
#[test]
fn all_bundled_drivers_and_declared_resource_files_are_readable_with_the_new_contracts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources");
    for name in ["coder", "reviewer"] {
        let manifest = root
            .join("agent-drivers/local")
            .join(name)
            .join("latest/driver.toml");
        let driver = agent_driver_core::read_driver(&manifest, &Default::default()).unwrap();
        mlua::Lua::new()
            .load(&driver.program)
            .into_function()
            .unwrap();
        for locator in driver.manifest.dependencies {
            let parsed = address::ParsedAddress::parse(&locator).unwrap();
            match parsed.kind.as_str() {
                "tool" => {
                    let path = address::resolve_file(&root.join("tools"), &locator).unwrap();
                    let tool = tool_resource_core::read_tool(&path, &Default::default()).unwrap();
                    mlua::Lua::new()
                        .load(&tool.program)
                        .into_function()
                        .unwrap();
                }
                "prompt" => {
                    let path = address::resolve_file(&root.join("prompts"), &locator).unwrap();
                    prompt_resource_core::read_prompt(&path, &Default::default()).unwrap();
                }
                "sandbox" => {
                    let path = address::resolve_file(&root.join("sandboxes"), &locator).unwrap();
                    sandbox_resource_core::read_sandbox(&path, &Default::default()).unwrap();
                }
                other => panic!("unexpected dependency kind: {other}"),
            }
        }
    }
}
