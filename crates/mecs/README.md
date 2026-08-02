# mecs

## 介绍

mecs 是一组领域无关的 Rust ECS 与基础设施 crate，目标是从 Margatroid 独立出来，作为可以
单独使用和发布的工具包。Core 提供最小 ECS，其余能力通过 Plugin 组合。

Core 遵循 KISS，只保留 Entity、Component、Resource、Event、System、Schedule、World 与
App。运行循环、异步执行、日志、信号和网络都属于独立 Plugin。判断一项能力是否属于 Core，
不看当前产品是否需要，而看移除它后 ECS 是否仍然成立。小 Core 的价值不是代码少，而是基本
规则少：对象由谁持有、数据何时可见、System 何时运行，都能从有限的结构直接推导。

公开 API 先按最方便且最自然的使用方式设计，再用接近 Rust 的权威伪代码写清类型、函数、
持有关系和执行逻辑，最后实现。开发者面对具体类型、泛型和普通 Rust 闭包，事件进入队列、
任务进入执行器后才在内部擦除。

每个子 crate 的 `README.md` 负责介绍和使用方式，`DESIGN.md` 是实现必须对应的权威伪代码。
