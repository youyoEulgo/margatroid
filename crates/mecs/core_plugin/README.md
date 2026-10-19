# CorePlugin

## 介绍

`core_plugin` 提供 mecs 的最小 ECS：Entity、Component、Resource、Event、System、Schedule、
World 与 App。它不知道 Runtime、异步执行、日志、信号和网络。

Core 遵循 KISS，只定义所有上层 Plugin 共同依赖且无法继续下沉的基本规则。事件发送不要求
配置期注册；具体事件首次到期时自动建立本帧读取存储，从未出现过的类型返回空读取器。

公开 API 保留具体类型和普通闭包，统一存储时才在内部擦除。Core 的 `emit_event` 只负责入队，
不猜测应用是否存在运行循环，也不产生隐式唤醒。

同步事件传递具体类型，System 则由开发者传入闭包。System 在本帧发送的事件要到下一帧才能
读取，读取器只访问本帧中对应类型的事件。读取返回 `&E`，不会消费事件；同帧多个 System
可以独立读取同一事件。Core 不提供通用事件 `take` API；闭包、异步请求或支流句柄中显式设计的
一次性所有权转移属于各自包装类型的内部约束。

`App` 拥有 `World` 和 Schedule，`World` 拥有实体、组件、按类型唯一的 Resource 及事件存储。
Resource 是当前 World 的单例，不是进程全局业务状态。`Entity` 同时包含索引和 generation；
删除后复用索引会推进 generation，旧标识无法访问新实体，generation 耗尽的索引永久退役。
实体标识仅在所属 World 内使用，不是跨 World 或持久化的业务 ID。

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
