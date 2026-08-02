# SignalPlugin

## 介绍

`SignalPlugin` 监听配置的操作系统进程信号，将其转换为类型化 ECS 事件并唤醒 Runtime。
它是输入桥接，不是关闭管理器；关闭、重载、暂停或忽略由读取事件的 System 决定。句柄可以
显式停止监听，未显式停止时由 RAII 完成回收。

## 使用说明

```rust
use app_runtime_plugin::RuntimePlugin;
use core_plugin::App;
use signal_plugin::{ProcessSignal, SignalOptions, SignalPlugin};

let mut app = App::new();
app.add_plugin(RuntimePlugin::default())
    .add_plugin(SignalPlugin::with_options(
        SignalOptions::new().with_signals([ProcessSignal::Interrupt, ProcessSignal::Terminate]),
    ));
```

`RuntimePlugin` 必须先安装。默认配置监听 `Interrupt` 与 `Terminate`，通常分别对应
`Ctrl+C/SIGINT` 和服务管理器发送的 `SIGTERM`。监听线程通过 `RuntimeEventSender` 直接
发送事件并唤醒事件驱动 Runtime。

## 处理信号事件

SignalPlugin 只通知，不执行关闭策略。业务或生命周期 Plugin 自行读取事件：

```rust
use app_runtime_plugin::RuntimePlugin;
use core_plugin::World;
use signal_plugin::{ProcessSignal, ProcessSignalReceived};

app.add_system(RuntimePlugin::UPDATE, |world: &mut World| {
    for event in world.event_reader::<ProcessSignalReceived>() {
        match event.signal {
            ProcessSignal::Interrupt | ProcessSignal::Terminate => {
                tracing::info!(?event.signal, "收到停止信号");
                // 在这里调用应用自己的优雅关闭策略。
            }
            ProcessSignal::Hangup => {
                // 可用于触发配置重载。
            }
            _ => {}
        }
    }
});
```

## 自定义监听信号

```rust
use signal_plugin::{ProcessSignal, SignalOptions, SignalPlugin};

let options = SignalOptions::new().with_signals([
    ProcessSignal::Interrupt,
    ProcessSignal::Terminate,
    ProcessSignal::Hangup,
    ProcessSignal::User1,
]);
app.add_plugin(SignalPlugin::with_options(options));
```

重复信号会按输入顺序去重，空列表会立即终止配置。Unix 可以使用
`ProcessSignal::Raw(number)`，但 `SIGKILL`、`SIGSTOP` 和非正数无法注册。非 Unix 平台
当前只保证 `Interrupt` 与 `Terminate`。

## 生命周期与错误

第一次执行 `RuntimePlugin::STARTUP` 时才会注册操作系统监听器并创建
`mecs-signal-listener` 线程。启动失败会产生 `SignalListenerFailed` 事件，Plugin 不会自动
重试或关闭 Runtime。

可以通过 World 中的 `SignalHandle` 查询或提前停止监听：

```rust
use signal_plugin::SignalHandle;

// SignalPlugin 在第一次 tick 的 STARTUP 阶段启动监听线程。
app.tick();
let handle = app.world().get_resource::<SignalHandle>().unwrap().clone();
assert!(handle.is_running());
handle.shutdown();
assert!(!handle.is_running());
```

`shutdown` 可以重复调用。App 销毁时，最后一个 `SignalHandle` 自动关闭 signal-hook
迭代器并回收监听线程。

## 公开 API

- `SignalPlugin`：插入 Handle 并添加 STARTUP System。
- `SignalOptions`：选择需要监听的进程信号。
- `ProcessSignal`：跨平台稳定信号枚举。
- `ProcessSignalReceived`：成功捕获信号时产生的事件。
- `SignalListenerFailed`：监听器启动失败事件。
- `SignalHandle`：查询或提前停止监听线程。

SignalPlugin 不处理终端按键；`Ctrl+C` 生效是因为终端通常把它转换为 `SIGINT`。它也不
直接关闭 Runtime、Server 或 Workspace。

完整伪代码与平台边界见 [DESIGN.md](DESIGN.md)。
