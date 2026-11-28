# lua_runtime_plugin：监督独立嵌入 VM

`lua_runtime_plugin` 把 [lua_runtime_core](../core/README.md) 的独立 VM 协议接入 ECS 应用。
它组合 Workspace、地址、Agent、存储、MCL 与任务 core，持有线程监督资源并完成启动和停止回复。
工具 runner 的执行监督入口见 [tool_plugin](../../tool/plugin/README.md)。
[Cargo 清单](Cargo.toml) 展示依赖，[lib](src/lib.rs) 展示公开插件配置。

## 围绕这些对象接入

| 对象 | 接入与生命周期职责 |
| --- | --- |
| `LuaRuntimePlugin` | 配置调度与 VM 预算，安装启停及退出处理 System |
| `LuaRuntimeConfig` | 保存内存、源码、启动等待与 hook 配置 |
| `StartDriverLuaVm` | 传入 owner、workspace、程序、环境及启动回复端 |
| `DriverLuaVmInstance` | 用完整 owner Entity 和 start_id 标识一次启动 |
| `DriverLuaVm` | 挂在执行宿主上的 ready、wake、ended 与取消组件 |
| `StopDriverLuaVm` | 请求结束指定实例，等待实际退出与线程 join |
| `LuaServices` | 由应用安装，使用真实执行身份转交业务操作 |

## 从应用安装到启动回复

完整应用安装见 [运行测试](tests/runtime.rs) 的 `Fixture::new`：
先安装 `RuntimePlugin`、`ClosurePlugin` 及 closure system、`AsyncRuntimePlugin`，
随后使用下面的公开入口安装独立 VM 处理：

```rust
use core_plugin::App;
use lua_runtime_plugin::{LuaRuntimeConfig, LuaRuntimePlugin};

fn install_lua_runtime(app: &mut App) {
    app.add_plugin(
        LuaRuntimePlugin::new()
            .with_config(LuaRuntimeConfig::default())
            .unwrap()
    );
}
```

调用方建立实际 owner 与 Workspace，安装 `LuaServices`，再发送 `StartDriverLuaVm`。
[同一测试](tests/runtime.rs) 的 `start_request` 给出 program、environment 与 oneshot 回复端的完整构造。
宿主服务或事件适配器在初始化完成后，以本次 `DriverLuaVmInstance` 调用 `WorldLuaRuntimeExt::driver_ready`，启动请求随之收到对应实例。测试中的Fixture通过自定义MCL处理器发送该通知。
随后 `margatroid:wait(target)` 通过服务等待目标唤醒，调用方可用该实例提交停止请求。

## 跟随一次 VM 生命周期

1. 启动处理校验 owner、Workspace、地址与环境，为本次执行建立唯一 start_id。
2. owner 挂载 `DriverLuaVm`，`VmThreads` 保存受监督线程，线程内建立当前线程 Tokio Runtime 和 LocalSet。
3. Lua 加载程序并调用 configure，配置确认后开放宿主方法并运行 execute。
4. ready 等待完成后交付启动实例；启动超时或失败进入取消与真实收口。
5. 停止处理关闭 wake 并请求取消，等待 ended、进程与日志收口，以及线程 join。
6. 收口完成后移除组件和线程记录；自然退出沿同一收口路径处理。

启停事件与内部结果的调度入口见 [system](src/system.rs)，处理流程见 [handler](src/handler.rs)，线程监督见 [types](src/types.rs)。
多个线程等待者共享 join 结果；收口失败时组件和控制能力继续由监督者持有。

## 业务身份怎样保留

owner 表示实际执行宿主，`LuaExecutionIdentity` 固定 source、workspace、address、instance 和取消 token。
业务服务据此验证来源；Lua context 作为脚本可读快照提供参数、资源目录与任务地址。
只有 owner 的真实任务集合中 execution_id 匹配本次执行时，plugin 才捕获任务能力，
并通过 `execution_finished` 等待其任务创建交接与子 Agent 清理。
独立 VM 自身的 ready/wake 服务于嵌入入口，业务 Agent 等待由 `LuaServices` 转交实际 AgentWakeSlot。

## 继续查接口与示例

- 两阶段参数和 context 快照：[运行测试](tests/runtime.rs) 的 `standalone_configure_and_execute_receive_real_arguments_and_context`。
- 自然退出、重复停止、旧实例与线程 join：[运行测试](tests/runtime.rs)。
- 文件、数据、进程与 wait_any：[运行测试](tests/runtime.rs) 的 `driver_can_use_shared_data_files_processes_and_wait_any`。
- MCL 返回值和错误值处理：[运行测试](tests/runtime.rs) 的 `mcl_failures_are_values_and_same_vm_continues_until_explicit_stop`。
- 共享 host、HTTP 和值桥接：[core 开发入口](../core/README.md)。
