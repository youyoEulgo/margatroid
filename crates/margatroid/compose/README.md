# compose

`compose` 把Margatroid各领域装配成完整后端。嵌入应用、开发新的启动入口或编写端到端测试时，可通过这个crate创建应用并管理退出。

## 创建后端

`ApplicationOptions` 提供两个路径：`data_root` 指向准备好的数据目录，`runner_path` 指向实际工具运行器。主程序将自身作为运行器，独立的 `tool_runner` 也实现相同的私有进程入口。

```rust,no_run
use compose::{build_app, ApplicationOptions};
use std::path::PathBuf;

fn create_backend(
    data_root: PathBuf,
    runner_path: PathBuf,
) -> Result<core_plugin::App, compose::ComposeError> {
    build_app(ApplicationOptions { data_root, runner_path })
}
```

装配读取 `config.toml` 和 `models.toml`，取得监听配置、工具预算及模型路由，再安装运行时、HTTP服务、领域插件和Web UI。数据目录中的资源根分别指向Driver、工具、提示词和沙箱目录。

应用启动入口负责创建初始目录，安装器负责部署配置及资源。[主程序](../../../apps/margatroid/src/main.rs)展示了创建目录、运行应用、打印地址和响应退出信号的完整流程。

## 运行与退出

`build_app` 返回 `core_plugin::App`。宿主通过 `AppRunExt` 驱动运行，或在测试中调用 `App::tick` 推进调度。中止控制在PRE_UPDATE运行，MCL、工具和推理在后续调度受理。

同步运行循环返回后，在启用time driver的Tokio运行时中等待关闭完成。同步宿主可使用以下函数：

```rust,no_run
fn close_backend(app: &mut core_plugin::App) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(compose::shutdown_app(app))?;
    Ok(())
}
```

`shutdown_app` 关闭HTTP受理门，持续推进World，依次等待初始化加载、Workspace与Agent创建交接、工具和推理监督、来源监控及数据库关闭。返回的 `ComposeError` 包含领域、路径和错误说明，宿主可据此呈现退出状态并继续监督清理。

## 装配中的对象关系

一个Workspace关联项目目录，并通过数据库管理Agent登记。Driver任务创建Agent，Agent再持有自己的MCL队列、推理句柄、工具任务和存储。停止创建任务会等待它绑定的Agent完成关闭。

同步绑定在领域插件安装前创建，随后将共享Clients集合交给状态探针和异步来源。记录监控使用存储的真实提交水位，配置及UI监控关联实际文件来源。

这些关系在 [handler.rs](src/handler.rs) 的安装与退出流程中集中体现。各领域的行为入口分别见 [Workspace](../workspace/README.md)、[Agent](../agent/README.md)、[工具](../tool/README.md) 和 [DTO](../dto/README.md)。

## 示例与参考

- [公开创建和关闭接口](src/lib.rs)
- [装配参数](src/types.rs)、[错误类型](src/error.rs)
- [应用生命周期测试](tests/lifecycle.rs)
- [WebSocket集成示例](tests/dto_websocket.rs)
- [MCL集成示例](tests/mcl_commands.rs)
- [实现设计](DESIGN.md)
