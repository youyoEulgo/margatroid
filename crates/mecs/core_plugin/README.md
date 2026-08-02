# CorePlugin

## 介绍

`core_plugin` 提供 mecs 的最小 ECS：Entity、Component、Resource、Event、System、Schedule、
World 与 App。它不知道 Runtime、异步执行、日志、信号和网络。

Core 遵循 KISS，只定义所有上层 Plugin 共同依赖且无法继续下沉的基本规则。事件发送不要求
配置期注册；具体事件首次到期时自动建立本帧读取存储，从未出现过的类型返回空读取器。

公开 API 保留具体类型和普通闭包，统一存储时才在内部擦除。Core 的 `emit_event` 只负责入队，
不猜测应用是否存在运行循环，也不产生隐式唤醒。

同步事件传递具体类型，System 则由开发者传入闭包。System 在本帧发送的事件要到下一帧才能
读取，读取器只访问本帧中对应类型的事件。

## 使用说明

```rust
use core_plugin::{App, Event, World};

struct Notice(&'static str);
impl Event for Notice {}

let mut app = App::new();
app.add_schedule("update".into())
    .add_system("update", |world: &mut World| {
        for notice in world.event_reader::<Notice>() {
            println!("{}", notice.0);
        }
    });

app.world().emit_event(Notice("hello"));
app.tick();
```

没有 RuntimePlugin 时由调用方主动调用 `tick`。安装 RuntimePlugin 后应改用它提供的
`send_event`，使事件入队时同时唤醒运行循环。

完整类型、函数和执行逻辑见 [DESIGN.md](DESIGN.md)。
