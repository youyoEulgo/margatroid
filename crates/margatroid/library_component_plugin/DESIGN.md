# 伪代码格式
```text
模块：使用一级标题，只写当前设计涉及的部分

类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束；单个约束写在同一行
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明

函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束；单个约束写在同一行
    行为：展开完整逻辑

逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
注释：字段注释使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# 板块格式

每个 crate 的 DESIGN.md 在伪代码格式之后使用六个一级标题，按 `lib`、`system`、`handler`、`events`、`types`、`error` 顺序组织。每个标题对应 `src/` 下的同名 Rust 文件：

```text
# lib        src/lib.rs        图书馆组件与 Plugin
```

本 crate 只有 lib：图书馆组件的标记 trait 与 Plugin，没有 System、Handler、事件、类型或错误。

# lib

lib 只放图书馆组件标记和 Plugin。

图书馆组件是 Entity 必须挂载的领域组件，组件存在本身表明 Entity 的领域身份。标记 trait 是这一定义的唯一判据：

```text
一个 Component 只有实现了 LibraryComponent，才能被称为图书馆组件
```

标记本身不含任何数据与方法，也不参与运行时判断；它的作用是让「图书馆组件」从文档约定变成编译器可见的事实，避免再靠 DESIGN.md 的措辞去推断某个组件算不算。

## 类型

公开：
```text
LibraryComponent：图书馆组件标记 trait，公开 trait
    约束：Component
    说明：继承 Component，因此只有组件才能被标记；空实现，无关联项与方法

LibraryComponentPlugin：图书馆组件标记 Plugin，公开类型
    说明：仅安装标记 Resource，不注册 System

LibraryComponentPluginInstalled：Plugin 安装标记，公开 Resource
    impl Resource for LibraryComponentPluginInstalled
```

## Plugin 行为

```text
impl Plugin for LibraryComponentPlugin
    Plugin：公开 trait 实现
    build(self, app: &mut App)
        安装：若已安装 LibraryComponentPluginInstalled 则 panic
        行为：插入 LibraryComponentPluginInstalled 标记 Resource
```

## 已标记的图书馆组件

每个 Entity 只挂载一个图书馆组件，组件与 Entity 一一对应：

```text
Agent：agent_plugin，Agent Entity 必须挂载 Agent + ResourceId
Workspace：workspace_plugin，Workspace Entity 必须挂载 Workspace + ResourceId
Client：client_plugin，Client Entity 必须挂载 Client + ResourceId
AgentImage：agent_image_loader_plugin，AgentImage Entity 必须挂载 AgentImage + ResourceId
```

Agent 的工具路径与推理配置不作为独立组件：项目根与镜像根属于 Agent.info，模型与上下文窗口属于 Agent.info.model，采样参数由 AgentImage 提供。曾存在的 AgentToolEnvironment 与 AgentInferenceSnapshot 无人挂载也无人查询，已删除。

以下不属于图书馆组件：

```text
ResourceId：全局标识值类型，被大量用作 map 键、结构体字段与跨边界序列化；
    它确实会被挂载到 Entity 上，但挂载只是它的一种用途，其身份首先是值类型而非领域组件
Health、Name、Selected：core_plugin 自带的内置组件，不属于 margatroid 领域
```
