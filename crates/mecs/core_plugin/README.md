# core_plugin

`core_plugin` 提供 mecs 的同步 ECS：应用拥有 World 和调度阶段，System 在阶段内读取事件、查询组件并修改资源。适合构建由调用方逐帧推进的程序，也作为运行循环与异步插件的基础。

## 从一个事件开始

```rust
use core_plugin::{App, Event, Resource, World};

struct Notice(u32);
impl Event for Notice {}

#[derive(Default)]
struct Total(u32);
impl Resource for Total {}

fn main() {
    let mut app = App::new();
    app.world_mut().insert_resource(Total::default());
    app.add_schedule("update".into())
        .add_system("update", |world: &mut World| {
            let sum: u32 = world.event_reader::<Notice>()
                .into_iter().map(|notice| notice.0).sum();
            world.get_resource_mut::<Total>().unwrap().0 += sum;
        });
    app.world().emit_event(Notice(7));
    app.tick();
    assert_eq!(app.world().get_resource::<Total>().unwrap().0, 7);
}
```

先创建 Schedule，再按执行顺序注册 System。`tick()` 更新本帧事件读取存储，然后按 Schedule 注册顺序运行各阶段；同一阶段内的 System 串行执行。

## 核心对象

- `App`：装配插件、Schedule 与 System，提供 `tick()` 和 `fast_forward_tick()`。
- `World`：持有实体、组件、资源和事件队列，提供查询与修改入口。
- `Entity` / `Component`：实体标识与按实体保存的类型化数据。
- `Resource`：按 Rust 类型保存的 World 级单例。
- `Event` / `EventReader`：事件数据与当前帧的共享读取视图。
- `System` / `Schedule`：同步执行单元与有序执行阶段。
- `Plugin`：通过 `build(self, &mut App)` 装配应用能力。

## 数据与事件生命周期

`Entity` 包含索引与 generation。删除实体后，复用索引会推进 generation；实体标识的作用域是创建它的 World。资源随 World 持有，组件随所属实体持有。

`emit_event` 将事件加入队列。本帧 System 发送的事件在后续帧进入读取存储；多个 System 可在同一帧读取同一批 `&E`。读取存储每帧更新，需要长期使用的值由业务复制到资源或组件中。

`emit_pending::<T, E>()` 返回 `EventHandle<Result<T, E>>`；调用 `complete` 后，响应节点进入普通事件分发流程。安装运行循环后，可使用 `app_runtime_plugin::WorldEventExt::send_event` 同时入队并唤醒循环。

## 继续阅读

- [公开入口](src/lib.rs)、[App 装配与逐帧执行](src/app.rs)、[World 数据访问](src/world.rs)。
- [外部 crate 接入测试](tests/public_api.rs)：组件、资源、插件及独立 Schedule 的组合。
- [设计与完整接口](DESIGN.md)。
