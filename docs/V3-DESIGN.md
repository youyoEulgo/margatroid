# Margatroid V3 架构设计

状态：mecs基础设施已稳定，正在按领域Plugin设计Margatroid资源加载与运行能力

本文只描述产品结构和稳定概念。具体公开类型、执行逻辑与使用方式以对应 crate 中的
`DESIGN.md` 和 `README.md` 为准。

## 1. 产品目标

Margatroid 是 Docker-like 的多 Agent 编排与运行工具：

- AgentImage 类似容器镜像，是一个 Agent 可启动的静态资源集合。
- AgentInstance 类似运行中的容器实例，只在启动时读取镜像。
- Workspace 是由 Workspace 文件编排的一组 AgentInstance。
- Memory 类似持久化卷，但默认按项目和 Agent 自动分配。
- Workflow 是可组合、可扩展的高级 Skill，未来可以演化为独立语言。

CLI 面向用户，daemon 拥有运行状态、资源库和 Memory。用户不直接操作 daemon 进程。
V3 第一版是本地产品：CLI 与 daemon 在同一台机器上共享文件系统，不为远程资源上传、
项目目录同步或 Memory 回传预留 API。

## 2. 分层

```text
apps/
├── cli                 短生命周期本地控制端
└── daemon              产品组合根

crates/mecs/
├── core_plugin         同步 ECS
└── *_plugin            可复用基础设施

crates/margatroid/
├── types               Plugin共享的无运行行为领域值类型
├── protocol            CLI/daemon 共享 DTO
├── compose             Workspace 文件编译器
├── *_plugin            业务能力
└── defaults            官方默认 Plugin 组合

legacy/                 只读历史参考，不参与 workspace
```

依赖方向：

```text
apps -> margatroid plugins -> mecs plugins -> core_plugin
apps -> compose -> protocol
margatroid plugins -> protocol
margatroid plugins -> types
types -> core_plugin
```

protocol不依赖ECS，compose不依赖daemon，core不依赖任何具体Plugin。types可以依赖core的
Entity与Event，保存ResourceName、Message和AgentMessage等业务Plugin共享静态契约，但不依赖
任何Margatroid业务Plugin，也不承载CLI/daemon协议；protocol只保存跨进程DTO。

Margatroid Plugin按资源所有权和运行职责组合：

```text
mecs基础设施
-> AgentImageLoaderPlugin / ModelRouteLoaderPlugin
-> ToolPlugin / tool_definition_plugins/{SkillPlugin, WorkflowPlugin} / InferencePlugin / AgentPlugin / MemoryPlugin
-> WorkspacePlugin
```

AgentImage和模型路由拥有独立生命周期，因此由Loader读取并形成运行时对象。Skill和Workflow只在
工具调用中使用，不建立额外Loader事件层：各自Plugin定义Tool并拥有格式解析和执行语义，ToolPlugin
注入实例位置并调用它们。共享资源名称仍来自无业务行为的纯类型crate。

## 3. mecs

mecs 是可独立发布的同步 ECS 工具包。core 只拥有：

```text
App
├── World
│   ├── Entity / Component
│   ├── Resource
│   └── Event queue
├── Stage -> systems
└── event maintenance
```

System 始终同步接收 `&mut World`。异步、运行循环、日志、HTTP、信号和终端输入都是独立
Plugin。所有外部线程通过有界通道把数据送回主线程，不能持有 World 引用。

通用 Stage 固定为 Startup、First、Update、Last，不加入业务阶段。Plugin 按依赖顺序安装，
关闭时按逆序释放。

## 4. 产品运行结构

```text
margatroid CLI
  -> locate margatroid-workspace.yaml
  -> compose path
  -> Compose编译WorkspaceDefinition
  -> daemon HTTP API / protocol
  -> ServerPlugin
  -> StartWorkspace / ReloadWorkspace / StopWorkspace Event
  -> WorkspacePlugin
  -> 收集AgentImage / Inference / Tool / Memory信息
  -> Agent静态配置与默认/动态可见性
  -> CreateAgent Event
  -> Workspace Entity + Agent Entity
  -> Agent/Workflow/Memory plugins
  -> InferenceCommand / SandboxCommand
  -> async runtime
  -> Result Event
  -> product event stream
  -> CLI logs/status
```

HTTP 只是适配层，不能承载业务状态。业务 Plugin 不直接访问 Axum handler，ServerPlugin 也不
直接修改业务 Resource。

daemon 是当前产品组合根，默认安装 Runtime、AsyncRuntime、Log、Server、AgentImageLoader、
Inference、Tool、Memory、Agent 和 Workspace Plugin。它通过 `/ws` 接收 protocol 定义的
`workspace.start` 请求，并把结构化日志转发给连接中的 CLI；daemon 不解析 Workspace YAML，
也不处理 CLI 的 LLM 消息输入输出。

## 5. Agent 模型

### AgentImage

AgentImage 是 Agent 库中的最小可启动镜像，至少包含：

- soul
- 稳定 ModelId，通常使用具体模型名
- 镜像内 Skill / Workflow
- 启动所需静态配置

镜像文件允许人工修改。Soul、ModelId、推理参数和资源可见名称不会自动修改运行中的
AgentInstance；更新这些静态配置需要通过`workspace reload`重新生成运行实例。已经可见的
Skill内容属于动态资源，不受此限制。
镜像名称采用 `scope/name:tag`。

AgentImageLoaderPlugin负责从主目录异步读取镜像并做结构验证，将其表示为AgentImage Entity。
Entity持有身份、Soul、中立模型配置和默认只读资源可见性；默认可见性只提供查询方法，不能被
其他Plugin原地修改。

WorkspacePlugin取得该Entity后，把镜像默认值与Workspace增删项合并，为新AgentInstance创建
最终资源可见性。InferencePlugin依赖Loader公开的中立模型配置，在实例创建时将其转换为
`ModelId`、推理参数与实例快照，并负责业务规则和路由验证。Loader不依赖任何业务Plugin。

AgentImage Entity刷新不会修改已有AgentInstance的Soul、推理快照或最终资源可见性；新配置只在
`workspace up/reload`创建的新实例上生效。Skill和Workflow正文由各自工具Plugin按调用读取，
ToolPlugin不理解它们的资源格式。

### AgentInstance

AgentInstance 属于一个 Workspace。Workspace 启动时收集 AgentImage 与额外资源信息，并将得到的
静态配置、`AgentDefaultVisibility`和`AgentDynamicVisibility`挂到 ECS Entity。实例之间不共享可变记忆。

### 模型路由

主目录 `~/.margatroid/models.toml` 是全局默认路由表，项目可以在
`<project>/.margatroid/models.toml` 定义 Workspace 级覆盖。同一 ModelId 按项目级、
全局的顺序查找；项目级路由挂在 Workspace Entity 上。AgentImage 和 AgentInstance 只
保存 ModelId，不持有 Provider、base URL 或 API key。

LLM 流式文本只用于前端实时显示，经有界通道直接转发，不为每个分片创建
ECS Event。后端内部累积完整 Assistant Message，只在响应完成时发布最终结果。

### Workspace

Workspace 是运行对象，不是 YAML 文件。YAML 是创建或更新 Workspace 所需的项目配置。
一个 Workspace 可以包含多个 AgentInstance，其中 manager 是用户和运行组的默认入口。

Compose把YAML编译为不包含运行时Entity的`WorkspaceDefinition`。WorkspacePlugin不解析YAML；它负责
加载全部AgentImage，从Inference、Tool和Memory等Plugin收集实例材料，全部准备成功后发送
`AgentCreateRequest`。AgentPlugin只创建自身组件并发布带请求ID的`AgentCreated`回执，WorkspacePlugin
再按回执绑定AgentMemory、AgentInferenceSnapshot和AgentToolEnvironment。Workspace Entity保存身份、
配置快照和Agent名称索引，Agent Entity通过`AgentWorkspaceId`反向关联所属Workspace。其他Plugin查询
各自的typed Component，不通过Workspace维护通用属性字典。

`workspace reload`先验证新定义与原Workspace身份一致，再关闭旧AgentInstance并按新定义创建新的
Workspace和Agent Entity。当前阶段不同时运行新旧实例，也不提供失败后的旧实例回滚。

## 6. Skill 与 Workflow

Skill 是带作用域的目录包，不是单 Markdown 文件。目录可以包含说明、脚本、模板和其他资源。

Workflow 属于 Skill 范畴，但负责显式控制多步骤执行。节点类型必须可扩展；未来可以增加条件、
循环、并行、人工确认、提示词注入和强制 Skill 调用，不能把第一版节点 enum 当作永久封闭集合。

AgentInstance持有两层统一资源可见性：`AgentDefaultVisibility`是Workspace创建时根据AgentImage
默认值和Workspace参数合并出的只读`ResourceRef`集合；`AgentDynamicVisibility`初始复制基线，
表示普通Tool、Skill、Workflow和未来资源的当前实际可用集合，后续可由Agent或Workflow逻辑调整。

每次LLM请求前，AgentPlugin遍历动态可见性的`ResourceRef`集合，将每个`ResourceRef`分别交给
ToolPlugin构造`Tool`，再收集definitions写入请求`tools`字段。ToolPlugin不接收资源集合，也不读取
Agent可见性组件。

前端可以随用户消息直接指定Skill、Workflow或其他工具调用。此时AgentPlugin先记录用户消息并执行
指定调用，不立即发送LLM请求；Tool响应作为统一Message写入上下文，全部指定调用完成后再使用完整上下文发起推理。
前端没有指定调用时，记录用户消息后直接推理。两条路径的LLM请求都从动态可见性构造`tools`，
用户消息意图不负责启用或禁用模型工具。

模型一次返回多个ToolCall时，AgentPlugin在`AgentStatus`中保存该批次的全部调用ID。每个Tool响应
只追加上下文并移除自己的ID，最后一个响应到达时才发送下一次`InferenceCommand`，避免同一批工具
响应触发多次推理。

SkillPlugin为每个可见Skill生成一个独立Tool，并在执行时按作用域动态解析内容：

```text
项目级 .margatroid > AgentImage内置 > 主目录 ~/.margatroid
```

作用域越窄优先级越高。最高优先级同名资源存在但内容非法时返回错误，不静默降级。修改已有
可见Skill的`SKILL.md`、脚本、模板或资产会在下一次使用时生效；默认ResourceRef的变化需要重载
Workspace，动态可见性变化从下一次LLM请求起生效。

Workspace中的Agent配置形式：

```yaml
agents:
  coder:
    image: local/coder:latest
    resources:
      - provider: skill
        name: local/project-context
      - provider: tool
        name: builtin/read-file
    disable_resources:
      - provider: skill
        name: local/dangerous-command
```

WorkflowPlugin同样把每个可见Workflow生成为独立Tool。Workflow依赖Skill时将对应Skill
`ResourceRef`加入动态可见性，通过同一Provider与Tool构造链路调用，不建立旁路加载协议。

## 7. Memory

主目录与项目级目录严格分开：

```text
~/.margatroid/                              全局资源与 daemon 数据
<project>/.margatroid/                      项目级数据
<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
```

默认情况下无需配置 memory path。相同 AgentImage 在不同项目 Workspace 中自然使用不同项目级
目录。显式 volume 只用于用户确实需要覆盖默认位置的场景。

每个逻辑Agent使用一个独立SQLite文件，包含两张业务表：

```text
history_messages
    追加已经提交的User、Assistant和Tool消息
    每行分列保存role、content、tool_calls、tool_call_id、交互轮次ID和时间
    上下文压缩不会删除或覆盖历史行

realtime_messages
    使用conversation和tool两个context分区保存AgentContext.messages和tool_context
    每次任一上下文变化后整体同步两个快照
    未来上下文压缩可以替换该表，但不影响history_messages
```

Skill正文只进入当前轮`tool_context`，不写入历史。Skill Tool的历史内容替换为
`skill: <scope/name> loaded`；普通Tool响应原样写入历史。历史表不再设置独立的资源列。

WorkspacePlugin负责确定数据库路径。`workspace up/reload`创建Agent前先由MemoryPlugin打开数据库
并读取`realtime_messages`，再把恢复出的`messages`和`tool_context`直接放入Agent创建事件。
无法读取已有数据库时启动失败，不能静默退化为空上下文。

`AgentContext.messages`通过`append_message`和`rewrite_messages`修改，`tool_context`通过追加和清空入口修改。修改完成后
都发送携带两个完整快照的`margatroid_types::AgentContextMessagesUpdated`；MemoryPlugin逐个消费事件，并各自使用一次
SQLite事务整体重写`realtime_messages`。MemoryPlugin仍不决定压缩时机、摘要内容、记忆检索或
上下文裁剪策略。

AgentPlugin的`AgentToolCallSystem`收到合法`AgentMessage`后，一律发送历史写入事件，不直接调用SQLite。
User和Assistant追加到长期`messages`，Tool追加到当前轮`tool_context`。`tool_context`只在收到下一条
User或Assistant时清空；`rewrite_messages`只影响实时表，不向历史表补写压缩结果。

## 8. CLI

CLI 是短生命周期本地控制端，它负责：

- 查找 Workspace 文件，调用共享 Compose 编译器，并将 `WorkspaceDefinition` 发送给 daemon。
- 发送 Workspace 与资源管理命令。
- 展示日志、状态和诊断。
- 为 `workspace config` 等纯预览命令调用共享 Compose 编译器。

当前 CLI 第一阶段只实现 `workspace up`：编译 Workspace 文件、通过 WebSocket 发送
`workspace.start`，并打印 `type = "log"` 的后端日志事件。CLI 暂不读取用户输入，也不处理
LLM 消息输入输出。

daemon 负责：

- 接收 Compose 编译后的 `WorkspaceDefinition`，做运行时复核和权威安全校验。
- 读取项目级 Skill / Workflow 和主目录已安装资源。
- 安装、更新、删除和查询主目录资源。
- Workspace、AgentInstance、Request 和 Task 状态。
- 持续读写项目级 Memory。
- 权威校验、持久化和安全策略。
- ECS 运行与恢复。

CLI 不打包资源正文，不维护资源副本，也不接收 Memory 增量回传。CLI 退出不影响
Workspace；daemon 负责完整运行生命周期。Agent默认可见性在启动时确定，动态可见性可在运行时改变；Skill等
动态资源在使用时读取，Memory由daemon在Workspace运行期间持续读写。

CLI 只参考 Docker 的管理方式，不复制容器的完整生命周期。`workspace up` 创建
运行组，`workspace down` 结束并移除运行组，`workspace reload` 按当前配置重新
创建运行组。不提供 pause、continue、start、stop 或 restart。`run` 从单个 AgentImage
创建临时 Workspace，`ps` 查询 daemon 权威状态。

## 9. 伪代码优先的API设计

API先按最方便、最自然的使用方式设计，再讨论如何实现。不得因为当前实现困难、内部类型结构
或既有代码习惯，提前把复杂度转嫁给调用者。

设计完成后，使用接近Rust且可一比一实现的伪代码描述类型、函数和运行逻辑。伪代码是实现契约，
不是方向性草稿；真实代码必须与通过审查的伪代码对应。发现错误、实现障碍或更优方案时，先更新
讨论结果和伪代码，再修改实现，不能自行偏离文档。

### 9.1 先设计调用方式

第一步只从调用者视角写API，不先考虑内部如何存储、类型擦除、加锁或跨线程。

优先回答：

```text
调用者想完成什么
最短、最自然的调用形式是什么
哪些参数必须由调用者提供
哪些信息可以由Plugin、World或上下文自动获得
返回值或响应如何被继续使用
```

公开API应减少不必要的注册、重复配置和手动中转。能够由已有上下文可靠推导的内容，不要求
调用者重复传入。能够沿用Rust和生态库既有习惯的能力，不发明另一套等价用法。

KISS约束作用于调用者需要理解的概念，而不是机械追求最少结构体。确实表达不同状态或所有权的
类型可以存在；仅为迎合当前实现而产生的公开中间类型不应进入API。

### 9.2 再写伪代码

API用法确定后，按Rust能够直接表达的方式写伪代码。公开泛型保留具体类型信息，内部实现需要
统一存储时再做类型擦除。

类型、字段、泛型、参数和返回值使用最终准备采用的Rust名称与类型；中文只负责解释语义。

```text
LogPlugin：日志插件，公开结构体--保存日志配置
    level: LogLevel--日志级别，私有
    with_level(mut self, level: LogLevel) -> Self
        设置级别：公开方法，level替换当前日志级别
        行为：保存level并返回自身
```

规则：

- 类型说明写明中文名称、可见性和用途。
- 每个字段必须写标准Rust字段名与类型，注释使用`--`附着。
- 方法写在所属类型下；与对象状态直接相关的操作优先设计成方法。
- 只放不属于任何类型的操作到“函数”板块。
- 方法和函数签名使用标准Rust名称、参数、泛型与返回类型。
- 无普通返回值时省略`-> ()`。
- 单行能够写完的约束并入同一行，复杂约束单独写“约束”。
- 自定义方法必须写完整行为；标准库Trait的常规行为可以用一句话概括。
- 不写Rust Attribute，实现时根据真实代码判断。
- 注释不能伪装成字段、参数或逻辑步骤。

### 9.3 Crate设计文档结构

每个crate的`DESIGN.md`只使用以下四个板块，并保持固定顺序：

```text
# CrateOrModuleName

## 类型
## 函数
## 逻辑
## 持有关系
```

“类型”按公开、crate公开和私有分组，记录字段、方法、Trait实现及其行为。

“函数”同样按可见性分组，只记录不属于某个类型的函数和System。

“逻辑”按实际执行顺序串联类型、方法、函数、事件和System。这里需要写清正常路径、必要分支
与失败路径，使实现者可以顺着伪代码复刻行为。

“持有关系”放在最后，以对象树展示World、Resource、Entity、Component、Handle、通道及临时
任务之间的所有权。它只解释谁持有什么，不重复职责说明或运行逻辑。

目标、非目标、使用说明、设计背景、职责讨论和教程不放进crate的`DESIGN.md`。需要保留给
使用者的内容写进README；跨crate的产品结构和稳定决定写进本文件。

### 9.4 沿依赖链逐步设计

从当前能力出发，只设计实现该API必需的依赖。涉及其他模块时，新增对应模块并写到当前已经
触及的边界，不提前补完整个未来系统。

顺序是：

```text
当前API
-> 当前类型与函数
-> 当前运行逻辑
-> 暴露出的直接依赖
-> 为直接依赖补最小API
-> 继续沿真实依赖向外辐射
```

这样可以从Event、World、System、Schedule、App等基础对象逐步扩展，也可以从AgentImage、
Skill等业务资源向其加载和运行依赖扩展。尚未被当前逻辑触及的能力不占位，不为想象中的需求
提前增加公开API。

### 9.5 先写逻辑，再讨论实现取舍

API和伪代码逻辑确定后，再评估实现可行性：

- Rust借用和所有权能否直接表达。
- 同步System与异步任务之间如何转移数据。
- 锁、通道、事件和快照由谁持有。
- 泛型在哪个公开边界保留，在哪个内部边界擦除。
- 当前设计是否引入无法接受的性能、稳定性或维护成本。

实现约束与API冲突时，优先寻找不改变调用体验的内部方案。确实无法兼得时，明确列出方案的
好处、代价和行为差异，讨论后再修改API和伪代码。

### 9.6 实现必须对应文档

通过审查后，真实代码按伪代码实现：

```text
公开类型和字段语义对应
公开方法与函数签名对应
可见性对应
事件和System执行顺序对应
所有权与持有关系对应
错误、分支和状态变化对应
```

实现中不得静默增加另一条等价API，也不得因为“更安全”“更规范”或“更方便实现”自行改变
已经确定的行为。必须调整时先说明原因，取得结论后同步修改伪代码，再更新代码。

实现完成后检查：

- 文档中的公开项是否全部实现。
- 代码是否额外导出了文档没有的API。
- 逻辑顺序和失败行为是否一致。
- README示例是否只使用已设计的公开API。
- 持有关系是否与真实字段和所有权一致。

### 9.7 README与DESIGN的边界

`DESIGN.md`面向设计和实现，强调精确、可复刻。

README面向使用者，可以包含：

```text
介绍
关键机制
安装方式
快速使用
常见组合
错误与运行行为
```

README不能发明DESIGN中不存在的API。DESIGN也不重复README中的背景说明和教程。两者发生
冲突时，先确认预期API并更新DESIGN，再让README和实现同时对齐。

## 10. 非目标

V3 第一版不承诺：

- 远程 daemon、远程 Workspace 项目目录、资源上传或 Memory 同步。
- Agent 热更新。
- 分布式 ECS 或多进程 World。
- 任意代码的绝对安全隔离。
- Workflow 语言的最终语法。
- 旧 runtime API 兼容。
