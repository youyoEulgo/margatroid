# Margatroid V3 架构设计

状态：阶段 3 已完成，下一阶段为公开 API 收口

本文只描述产品结构和稳定概念。具体公开类型以 [MECS-API.md](MECS-API.md) 和
[MARGATROID-API.md](MARGATROID-API.md) 为准，执行顺序以 [V3-ROADMAP.md](V3-ROADMAP.md)
为准。

## 1. 产品目标

Margatroid 是 Docker-like 的多 Agent 编排与运行工具：

- AgentImage 类似容器镜像，是一个 Agent 可启动的静态资源集合。
- AgentInstance 类似运行中的容器实例，只在启动时读取镜像。
- Workspace 是由 Workspace 文件编排的一组 AgentInstance。
- Memory 类似持久化卷，但默认按项目和 Agent 自动分配。
- Workflow 是可组合、可扩展的高级 Skill，未来可以演化为独立语言。

CLI 面向用户，daemon 拥有运行状态和资源库。用户不直接操作 margatroidd。

## 2. 分层

```text
apps/
├── cli                 本地项目工具链和 daemon 客户端
└── daemon              产品组合根

crates/mecs/
├── core_plugin         同步 ECS
└── *_plugin            可复用基础设施

crates/margatroid/
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
```

protocol 不依赖 ECS，compose 不依赖 daemon，core 不依赖任何具体 Plugin。

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
  -> compile margatroid-workspace.yaml
  -> WorkspaceBundle
  -> daemon HTTP API
  -> ServerPlugin
  -> WorkspaceCommand Event
  -> WorkspacePlugin
  -> Workspace state + Agent Entity
  -> Agent/Workflow/Memory plugins
  -> LlmCommand / SandboxCommand
  -> async runtime
  -> Result Event
  -> product event stream
  -> CLI logs/status
```

HTTP 只是适配层，不能承载业务状态。业务 Plugin 不直接访问 Axum handler，ServerPlugin 也不
直接修改业务 Resource。

## 5. Agent 模型

### AgentImage

AgentImage 是 Agent 库中的最小可启动镜像，至少包含：

- soul
- provider 与 model 引用
- 镜像内 Skill / Workflow
- 启动所需静态配置

镜像文件允许人工修改，但运行中的 AgentInstance 不热更新；更新需要重启实例。
镜像名称采用 `scope/name:tag`。

### AgentInstance

AgentInstance 属于一个 Workspace。Workspace 启动时解析 AgentImage 与额外资源，并将得到的
运行快照挂到 ECS Entity。实例之间不共享可变记忆。

### Workspace

Workspace 是运行对象，不是 YAML 文件。YAML 是创建或更新 Workspace 所需的项目配置。
一个 Workspace 可以包含多个 AgentInstance，其中 manager 是用户和运行组的默认入口。

## 6. Skill 与 Workflow

Skill 是带作用域的目录包，不是单 Markdown 文件。目录可以包含说明、脚本、模板和其他资源。

Workflow 属于 Skill 范畴，但负责显式控制多步骤执行。节点类型必须可扩展；未来可以增加条件、
循环、并行、人工确认、提示词注入和强制 Skill 调用，不能把第一版节点 enum 当作永久封闭集合。

资源来源按作用域解析：

```text
AgentImage 内置 > 项目级 .margatroid > 主目录 ~/.margatroid
```

作用域越窄优先级越高。Compose 中列出的 Skill / Workflow 是该 Agent 除镜像内置资源外常态
可见的资源；存在于目录中不代表自动加载。

Workflow 依赖由包内依赖清单声明。启动前检查其 Skill 依赖，缺失时拒绝启动，不进行静默降级。

## 7. Memory

主目录与项目级目录严格分开：

```text
~/.margatroid/                              全局资源与 daemon 数据
<project>/.margatroid/                      项目级数据
<project>/.margatroid/workspaces/<workspace>/memory/<agent>/memory.sql
```

默认情况下无需配置 memory path。相同 AgentImage 在不同项目 Workspace 中自然使用不同项目级
目录。显式 volume 只用于用户确实需要覆盖默认位置的场景。

## 8. CLI

CLI 不只是 HTTP 转发器，它负责：

- 查找和解析 Workspace 文件。
- 解析项目相对路径。
- 收集项目级 Skill / Workflow。
- 构建确定性 WorkspaceBundle。
- 展示日志、状态和诊断。
- 管理可删除的本地缓存。

daemon 负责：

- 已安装资源的权威目录。
- Workspace、AgentInstance、Request 和 Task 状态。
- 权威校验、持久化和安全策略。
- ECS 运行与恢复。

CLI 命令语义参考 Docker，但不复制容易混淆的名词。`workspace up` 创建或启动运行组，`run`
从单个 AgentImage 创建临时 Workspace，`ps` 查询 daemon 权威状态。

## 9. API 设计方法论

API 是模块对调用者承诺的最小契约，不是内部对象的目录。设计目标不是单纯减少类型数量，
而是减少调用者必须理解的概念、选择和状态组合。

### 9.1 从场景开始，不从结构体开始

设计前先写出最多三个首版场景：

```text
普通场景    使用默认值完成核心操作
高级场景    修改确有需要的配置
失败场景    调用者如何识别失败并恢复
```

每个场景必须能写成一段最小调用示例。不能通过示例说明的类型暂不公开。不得先列内部
`Manager/Registry/Record/State/Handle`，再反推用户可能如何使用它们。

### 9.2 先确定边界

每项能力先回答：

1. 谁拥有权威状态？
2. 谁允许修改它？
3. 状态活多久，是 Entity、Workspace、App 还是进程级？
4. 调用跨不跨线程、进程或信任边界？
5. 移除该模块后，哪些调用者应当受影响？

回答不清楚时不能开始设计公开类型。一个对象不能同时由 CLI、daemon 和 Plugin 各维护一份
权威副本。

### 9.3 写出唯一主路径

先用一行数据流描述功能：

```text
Input -> Owner -> State change -> Result
```

例如：

```text
WorkspaceCommand -> WorkspacePlugin -> Workspaces -> WorkspaceResult
```

主路径中一次动作只能有一个写入口。Event、Resource 方法、Handle 方法和 HTTP handler 不能
同时成为同一动作的并列入口；adapter 只能转换输入，不能绕过 Owner。

### 9.4 给公开类型分类

每个公开类型必须属于下列一种；无法归类则保持私有：

| 类型 | 用途 | 不应用于 |
|---|---|---|
| Command | 请求 Owner 改变业务状态 | 表示已经发生的事实 |
| Result | 回答一次 Command 的成功或失败 | 保存长期权威状态 |
| Event | 通知多个消费者一个已经发生的事实 | 代替普通函数返回值 |
| Query/State | 提供只读快照或查询 | 暴露等价写方法 |
| Handle | 跨线程控制基础设施或外部资源 | 普通业务状态修改 |
| Component | Entity 私有数据 | World 级单例 |
| Resource | World 级共享状态或服务 | 任何方便共享的对象 |
| DTO | 固定进程/网络边界的数据形状 | 直接承载运行时行为 |
| Options | 聚合高级配置 | 为单个参数制造包装层 |
| Trait | 允许第三方替换或扩展行为 | 只有一个实现的内部抽象 |

结构体数量不是独立指标。一个 endpoint 一个 DTO 可以保持协议演化边界；同一操作拆成
Requested/Started/Completed/Failed 四个 Event，却没有四类消费者，就是无效复杂度。

### 9.5 Event 的准入条件

一个新 Event 至少满足一项：

- 有多个互不依赖的消费者。
- 发生时间本身有意义，消费者允许稍后读取。
- 需要跨 Stage 或外部线程回到主线程。
- 需要广播一个事实，而不是取得一次函数结果。

仅用于一个 System 串联下一个 System 的中间 Event 默认私有。`Started`、`Progress`、`Chunk`
只有被独立观察、取消或展示时才公开。Command 与 Result 使用同一个关联 ID，不依赖队列顺序。

### 9.6 Resource、Handle 与直接方法

选择顺序：

```text
同步局部操作          -> 普通方法
业务状态变更          -> Command / Result
World 级只读状态      -> Query Resource
跨线程基础设施控制    -> Handle
外部线程输入 ECS      -> External Event Sender
```

业务 Resource 默认只读。基础设施 Handle 可以有 `wake/cancel/shutdown` 等命令式方法，因为它
本身就是跨线程控制边界。不要再为同一 Handle 操作包装一层等价 ECS Event。

### 9.7 配置 API

配置分三层：

```text
Default                    常见场景零配置可用
Options                    高级配置集中管理
生态原生对象               专家场景直接接入 tracing/Axum 等
```

`new()` 只接收创建对象不可缺少的参数；无必填参数时使用 `Default`。不能同时提供语义相同的
`new()` 和 `default()`。Plugin 可以保留少量最高频快捷方法，高级字段只放在 Options，避免
Plugin 和 Options 各维护一套完整 builder。

配置值在进入运行期前验证。用户输入错误返回结构化 Error；只有违反开发者不变量时才 panic。

### 9.8 Error 设计

每个能力默认只公开一个 Error 类型，用 kind 或 enum variant 区分原因。Error 必须回答：

- 哪个操作失败。
- 调用者能否重试或修正输入。
- 哪个稳定分类可用于分支处理。
- 哪段安全 message 可以展示给用户。

底层库 Error、绝对路径、secret、完整 prompt 和无限制 stdout/stderr 不直接穿透公共边界。
日志提供诊断细节，Result 提供调用者需要处理的稳定信息。

### 9.9 同步、异步与生命周期

同步或异步是执行策略，不能让同一业务能力出现两套 API。业务仍是
`Command -> Result`；Future 不持有 World，完成后回流 Result Event。

生命周期只公开调用者需要决策的状态。内部 worker 的每一步启动和关闭不自动成为 Event。
依赖按正序安装、逆序关闭；只有无法由依赖顺序表达的真正边界，才增加类似
`after_shutdown` 的最小机制。

### 9.10 DTO 与领域对象

网络 DTO、Compose authoring 数据、Plugin Event 和内部存储模型必须分开：

```text
YAML authoring -> normalized protocol DTO -> Command -> domain state
```

它们可以字段相似，但兼容周期不同。DTO 允许为了版本演化保留薄包装；不能为了少一个结构体
让 HTTP 协议直接依赖 ECS Event，也不能让 daemon 依赖 CLI 私有解析类型。

### 9.11 稳定性预算

公开即兼容承诺。每增加一个 public item，都必须记录：

- 首个真实调用者是谁。
- 为什么现有 API 无法表达。
- 谁负责维护其不变量。
- 失败与并发语义是什么。
- 未来删除它会影响什么。

没有真实调用者、仅供测试、只包装内部依赖或只为未来猜测存在的类型不公开。未实现能力进入
路线图，不进入目标 API 文档。

### 9.12 标准设计流程

新 API 按以下顺序设计：

1. 写目标和明确非目标。
2. 写三个首版使用场景。
3. 确定 Owner、生命周期、线程和信任边界。
4. 画一条唯一主数据流。
5. 列出最小公开类型，并为每个类型分类。
6. 写成功、失败、取消和关闭语义。
7. 写默认调用示例和高级调用示例。
8. 删除不影响示例的公开类型。
9. 对照实现，确认 `lib.rs` 没有额外导出。
10. 通过 API 审查后再实现或扩展。

### 9.13 API 提案模板

```markdown
# <Capability> API

目标：
非目标：
Owner 与生命周期：
依赖与信任边界：

普通场景：
高级场景：
失败场景：

主数据流：Input -> Owner -> State -> Result

公开类型：
- Type：分类；首个调用者；公开原因

写入口：
只读入口：
错误语义：
并发/异步语义：
关闭与恢复语义：
暂缓能力：
```

### 9.14 审查清单

- 是否能删除一种调用路径而不损失能力？
- 是否存在 Event 和 Resource 方法两条等价写入口？
- 是否把内部生命周期或存储对象公开了？
- 是否每个公开类型都有当前调用者？
- 是否默认场景需要理解高级配置？
- 是否使用 `Default` 和 `new()` 表达了同一件事？
- 是否因实现当前是异步而改变了业务 API？
- 是否混用了 DTO、Event 和持久化模型？
- 是否能从名称判断对象的 Owner 和读写方向？
- 是否写清失败、取消、背压、关闭和恢复？
- 删除 Plugin 后，无直接依赖的模块是否仍然成立？
- `lib.rs` 实际导出是否与设计文档完全一致？

任一问题没有明确答案，API 继续保持草案，不进入稳定公开面。

## 10. 非目标

V3 第一版不承诺：

- 远程 Workspace 项目目录。
- Agent 热更新。
- 分布式 ECS 或多进程 World。
- 任意代码的绝对安全隔离。
- Workflow 语言的最终语法。
- 旧 runtime API 兼容。
