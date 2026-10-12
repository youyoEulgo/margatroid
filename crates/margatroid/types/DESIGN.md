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

本文为共享types的完整目标归属与值契约，尚未实现。各领域文档描述对应完整行为，本文件统一依赖方向和跨领域数据，不继续混入旧邮箱、引用Block或ResourceId组件方案。

# lib

## 逻辑

```text
依赖方向：
    共享types只依赖基础ECS、通用执行接口和纯值依赖，不依赖业务Plugin实现
    ResourceID和ResourceSet等基础契约必须位于共享层或共享层可依赖的纯基础crate
    Agent、Workspace、Client、资源条目及World扩展声明与实现定义在对应xxx_components.rs
    Plugin、System和Handler留在实现crate；文档仍在lib板块描述共享组件，不按源码迁移改变文档板块
    Provider公开输入与trait是共享纯契约，具体OpenAI/DeepSeek实现留在InferencePlugin
    具体SQLite、HTTP、Lua VM、runner和平台沙箱实现不进入共享组件
```

# system

## 逻辑

```text
无System，不做消息投递、任务轮询、GC、数据库同步或事件转发。
```

# handler

## 逻辑

```text
无业务Handler。共享World扩展的受检实现属于对应components文件，不能留在Plugin使消费者依赖实现crate。
AgentExecutionScope拥有活动受理、取消和完成等待的共享机制，不包含具体工具、HTTP或数据库执行器。
```

# events

## 逻辑

```text
事件归属：
    所有公开和内部事件定义于对应xxx_events.rs，插件不保留events.rs
    自动异步输出为框架Result<T,E>，T/E及其关联载荷也在共享events定义
    mcl_events：MclCommand、MclCommandResult、MclAsyncOutput及错误关联
    inference_events：InferenceRequest、InferenceResponse、配置重载请求及关联输出
    tool_events：ToolExecuteRequest、ToolExecuteResponse
    agent_events：启动/停止及AgentException、AgentExceptionResult
    workspace_events：启动/停止及WorkspaceException、WorkspaceExceptionResult
    lua_runtime_events：Driver启动/停止、异常及对应结果，ended仍是oneshot槽位不是额外事件
    各资源events：加载请求、一次性回复槽、公开观察Result及读取输出
    AgentImage保留Workspace依赖的加载Result；完成保障是私有实现，不建立跨插件LoadCompletion公共trait

完成通道：
    oneshot：一次请求完成，Clone事件共享同一个Arc<Mutex<Option<Sender>>>槽
    框架Result：闭包到ECS的结果收口，需要World回写时使用
    观察事件：不作为队列推进、清理或执行成功的依据
    异常链：报告处理事实，不替代请求结果，不消费Result后重新发同类异常造成循环

移除旧入口：
    删除旧AgentMessage路由事件、AgentFailure、AgentControl、Lua消息邮箱封装
    删除CapturedInference及ContextCompaction专用请求、旧ToolCallEvent和历史转发请求
    不保留RefBlock、RefMerge或ResourceRef作为目标值；迁移兼容代码不构成新契约
```

# types

## 类型

公开：
```text
ResourceID：资源地址，公开纯值类型
    说明：不是Component；图书馆身份存在组件中，资源ID仅存在ResourceSet的map key中。

ToolCall：工具调用输入，公开结构体
    id: String--单批次唯一的调用关联
    tool_name: String--模型看到的资源alias，不是全局身份
    arguments: String--完整JSON对象文本，空参数为{}
    impl Clone + PartialEq + Eq + Serialize + Deserialize for ToolCall

TokenUsage：单次推理统计，公开结构体
    input_tokens: u64
    output_tokens: u64
    cache_hit_tokens: u64--Provider未提供时为0

AgentMessage：消息值，公开枚举--不是Event，不包含路由Entity
    Inject { messages: Vec<AgentMessage> }
    User { content: String }
    Assistant { reasoning: Option<String>, content: Option<String>, tool_calls: Vec<ToolCall>, usage: Option<TokenUsage> }
    ToolCall { resource_id: Option<ResourceID>, tool_call_id: String, tool_name: String, content: String, failed: bool }
    Error { message: String }
    impl Clone + PartialEq + Eq + Serialize + Deserialize for AgentMessage
    说明：JSON type为inject/user/assistant/tool_call/error；未知资源的可见性拒绝结果resource_id为None，不虚构builtin/invalid地址。
        inject递归包含同一消息值类型，不设置更窄的内部白名单
        Assistant可只有合法工具调用而无正文，usage保留于结果及历史，不回传作为模型输入
        System不属于AgentMessage；Provider内部必要角色由ProviderMessage契约表达

BlockInner：字段数组，公开枚举
    Message(Vec<AgentMessage>)
    ResourceID(Vec<ResourceID>)
    ToolCall(Vec<ToolCall>)
    inner_type(&self) -> InnerType
        读取类型：公开方法
        行为：返回对应变体类型，空数组也有类型。
    len(&self) -> usize
        读取长度：公开方法
        行为：返回数组元素数量。
    impl Clone + PartialEq + Eq + Serialize + Deserialize for BlockInner

InnerType：字段类型，公开枚举
    Message
    ResourceID
    ToolCall

BlockPath：字段地址，公开结构体
    block_id: String
    inner_id: String

Block：普通字段容器，公开结构体
    inners: HashMap<String, BlockInner>

BlockAssembly：普通Block容器，公开结构体
    blocks: HashMap<String, Block>
    说明：不含RefBlock或RefMerge，机械操作由AgentBlock受检接口维护。

MclInstruction：指令内容，公开结构体
    text: String--原始指令文本
    arguments: Vec<serde_json::Value>--每项是一个数组，按问号出现顺序对应；无问号为空
    说明：单问号也不省略外层，数量和类型不符报错，不接受binding兼容层隐式猜测。

HistoryRecord：历史记录，公开共享契约
    key: String--完整Agent Entity、请求ID、类别和调用ID或index形成的幂等键
    source: String
    turn: Option<String>--执行开始时快照
    kind: String--mcl、assistant、tool_call及消息类别
    content: String--指令原文或消息正文
    payload: serde_json::Value--完整消息/usage/原指令及展开参数
    说明：相同key及相同内容重复追加为幂等成功，内容冲突返回错误；批量接口单事务全写或全不写。
```

## 逻辑

```text
共享组件：
    agent_components：Agent、AgentLua、AgentMclQueue、MclPending、AgentExecutionScope及WorldAgentExt
    execution_components：活动机制可单列共享文件，Agent只引用一个AgentExecutionScope，不再平行挂AgentExecutionActivities
    workspace_components：工作区身份、成员、执行线程入口及WorldWorkspaceExt
    resource_components：ResourceInner、ResourcesComponent、LibraryComponent、ResourceSet
    领域resource_components：五类资源及AgentImage，AgentImage不强制有alias
    client_components：Client和其连接/发送器受检查询契约

资源规则：
    ResourceSet按完整ID存放目标Entity本地条目，可跨Entity有同ID，不能全局取第一个匹配者
    可见集合中工具alias必须唯一，Inference和MCL工具准备均拒绝冲突
    IMPORT必须精确属于Agent镜像依赖，无builtin例外，加载插件不重复判断Agent许可
    重载成功替换对应键，失败保留旧条目；激活沙箱改变策略时遵守Sandbox活动互斥

值与输出：
    GET始终返回typed数组，INJECT左侧同规则，右侧为唯一写入范围
    inference完整返回InferenceResponse，保留stop_reason，不只抽取assistant正文
    MclCommandResult.result是真实执行结果，history_error单列执行后历史错误，不能因此重做工具
```

# error

## 逻辑

```text
错误归对应共享领域契约，不引用业务插件私有类型。
执行前历史失败阻止执行，执行后历史失败不覆盖真实结果；history_append本身失败正常回错误。
Cancelled、CleanupFailed、重复请求、alias冲突和幂等历史内容冲突必须有明确分类。
From<AsyncTaskError>不能凭空取得请求ID，闭包提交包装必须保存关联，不能仅日志后丢失回执。
```
