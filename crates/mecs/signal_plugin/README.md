# signal_plugin

`signal_plugin` 将配置的操作系统进程信号转换为类型化 ECS 事件，并唤醒运行循环。应用的 System 读取信号后执行关闭、重载或暂停流程。

## 安装并读取信号

```rust
use app_runtime_plugin::{AppRunExt, RuntimePlugin};
use core_plugin::{App, World};
use signal_plugin::{
    ProcessSignalReceived, SignalListenerFailed, SignalPlugin,
};

fn main() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(SignalPlugin::new())
        .add_system(RuntimePlugin::UPDATE, |world: &mut World| {
            for event in world.event_reader::<ProcessSignalReceived>() {
                println!("received {:?}", event.signal);
            }
            for failure in world.event_reader::<SignalListenerFailed>() {
                eprintln!("signal listener: {}", failure.message);
            }
        });
    app.run();
}
```

默认监听 `Interrupt` 与 `Terminate`。终端通常将 Ctrl+C 转换为 SIGINT；应用可在接收 System 中将信号接入自己的生命周期控制。

## 配置与公开对象

- `SignalPlugin`：安装资源并注册 STARTUP 监听启动 System。
- `SignalOptions`：通过 `with_signals` 选择信号，按输入顺序去重。
- `ProcessSignal`：命名信号枚举；Unix 还提供 `Raw(number)`。
- `ProcessSignalReceived`：包含捕获信号的 ECS 事件。
- `SignalListenerFailed`：包含监听器启动失败信息的 ECS 事件。
- `SignalHandle`：通过 `is_running()` 查询，通过 `shutdown()` 提前停止。

自定义配置使用 `SignalPlugin::with_options(SignalOptions::new().with_signals([...]))`。信号列表至少包含一项。Unix 的 Raw 值通过 signal-hook 注册规则校验；具体平台映射见[监听资源实现](src/resource.rs)。

## 监听生命周期

先安装 `RuntimePlugin`，再安装信号插件。首次 tick 的 STARTUP 阶段注册操作系统监听并启动 `mecs-signal-listener` 线程；启动错误通过 `SignalListenerFailed` 交给后续帧处理。

`SignalHandle::shutdown()` 关闭监听并回收线程，可重复调用。最后一个句柄释放时同样执行回收，外部克隆会延长监听生命周期。应用在销毁 Runtime 前完成监听停止。signal-hook 可合并同类信号，事件适合表达控制意图。

[信号转换测试](src/plugin.rs) 展示 Unix User1 信号到事件的完整路径；[选项实现与测试](src/options.rs) 展示信号选择；完整接口与平台映射见 [DESIGN.md](DESIGN.md)。
