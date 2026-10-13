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

**本轮共享契约已实现：Block仅保留MESSAGE/TOOL/SANDBOX三种typed数组，删除RESOURCE（ResourceID）/TOOL_CALL（ToolCall）数组；System消息、Agent全域alias绑定及不可变导入定义和策略快照均已迁移。ToolCall纯输入仍保存在Assistant.tool_calls，不是独立Block类型。**

共享值、组件与事件迁移及本轮typed数组与导入绑定均已实现。各领域文档描述对应完整行为，本文件统一依赖方向和跨领域数据，不继续混入旧邮箱、引用Block或ResourceId组件方案。ResourceID等基础契约由resource_id_plugin提供；types按组件、事件与纯值模块组织，不依赖业务Plugin实现。正文保持lib、system、handler、events、types、error六个标准板块。

显式message历史策略已实现：推理和工具调用只返回结果，所有message历史仅由显式MCL history_append控制。MCL指令审计仍自动记录，审计payload中的消息不作为独立message记录。示例Driver对需要保留的结果另发显式追加指令。

# lib

## 逻辑

```text
依赖方向：
    共享types只依赖基础ECS、通用执行接口和纯值依赖，不依赖业务Plugin实现
    ResourceID和ResourceSet等基础契约必须位于共享层或共享层可依赖的纯基础crate
    Agent、Workspace、Client、资源条目及World扩展声明与实现定义在对应xxx_components.rs
    Plugin、System和Handler留在实现crate；文档仍在lib板块描述共享组件，不按源码迁移改变文档板块
    Provider公开输入与trait目前位于InferencePlugin的types模块，只引用共享消息纯值；共享事件通过原始配置文本传递重载，不反向依赖这些协议实现
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
    System { content: String }--系统消息，prompt导入及推理按实际role保留
    User { content: String }
    Assistant { reasoning: Option<String>, content: Option<String>, tool_calls: Vec<ToolCall>, usage: Option<TokenUsage> }
    ToolCall { resource_id: Option<ResourceID>, tool_call_id: String, tool_name: String, content: String, failed: bool }
    Error { message: String }
    impl Clone + PartialEq + Eq + Serialize + Deserialize for AgentMessage
    说明：JSON type为inject/system/user/assistant/tool_call/error；未知资源的可见性拒绝结果resource_id为None，不虚构builtin/invalid地址。
        inject递归包含同一消息值类型；history_append的允许范围独立校验，不能从消息枚举自动推导
        Assistant可只有合法工具调用而无正文，usage保留于结果及历史，不回传作为模型输入
        infer接受System，prompt导入按资源实际role构造消息，不把所有prompt强制转换为System或User
        ToolCall结构体仍是Assistant.tool_calls的纯输入元素；AgentMessage::ToolCall是执行结果消息，两者都不构成独立TOOL_CALL Block或GET数组

CallableResource：可调用资源值，公开结构体--TOOL数组元素，不是裸ResourceID
    resource_id: ResourceID--完整来源身份
    alias: String--本次导入的模型可见alias快照
    definition: Arc<CallableDefinitionSnapshot>--共享不可变定义快照，不复制资源正文

CallableDefinitionSnapshot：可调用资源定义快照，公开枚举
    Tool(ToolResource)
    Skill(SkillResource)
    Hook(HookResource)
    说明：复用共享资源定义，包含执行准备所需描述、schema及资源定义；不得回查可变集合替换旧值的定义。
        快照固定描述、schema和资源定义字段（包括路径），不冻结路径对应磁盘文件内容；执行时按既有路径读取，不宣称文件正文版本已被快照。
        Hook是允许导入的TOOL资源类别，hook执行语义尚未确认，不能由此声明tool_call已支持hook执行。

SandboxValue：沙箱资源值，公开结构体--SANDBOX数组元素
    resource_id: ResourceID--完整来源身份
    policy: Arc<str>--共享不可变策略快照；资源重载不暗改此值

ImportedValue：导入绑定值，公开共享枚举
    Message { resource_id: ResourceID, messages: Arc<[AgentMessage]> }--prompt来源ID及实际role消息快照
    Tool(CallableResource)--来源ID由CallableResource持有，不再重复
    Sandbox(SandboxValue)--来源ID由SandboxValue持有，不再重复
    说明：不建立重复资源正文仓库；MESSAGE共享消息快照，TOOL/SANDBOX共享Arc定义或策略。

ImportBindings：Agent导入绑定，公开共享类型--随Agent保存，不是额外Component或全局注册表
    bindings: BTreeMap<String, ImportedValue>--私有字段，alias到typed值的唯一权威绑定
    get(&self, alias: &str) -> Option<&ImportedValue>
        只读绑定：公开方法，不绕过World受检写入。
    contains_resource(&self, resource_id: &ResourceID, kind: InnerType) -> bool
        检查来源：公开方法，只检查ID与kind；旧Block快照不要求与最新alias或定义相等。
    说明：alias首字符必须是ASCII字母或下划线，后续仅ASCII字母、数字、下划线或连字符；不接受点、空白或非ASCII字符。
        受检更新与读取由共享agent_components中的WorldAgentExt统一实施，MCL只调用该契约。
        不按MESSAGE/TOOL/SANDBOX分别维护alias表；Agent全域同alias不同完整ID（含跨类型）拒绝。
        同ID同kind重载成功才替换对应alias的新值，失败保留旧绑定；重载alias改变时先查新alias冲突，成功时移除旧绑定再发布新绑定，不留下同来源不同alias分叉。
        已取出的Block/GET值仍持有旧快照；Agent.sandbox选择SandboxValue的ID及policy快照，重载不暗改激活值。

BlockInner：字段数组，公开枚举--只保留三种typed数组
    Message(Vec<AgentMessage>)--MESSAGE
    Tool(Vec<CallableResource>)--TOOL
    Sandbox(Vec<SandboxValue>)--SANDBOX
    inner_type(&self) -> InnerType
        读取类型：公开方法
        行为：返回对应变体类型，空数组也有类型。
    len(&self) -> usize
        读取长度：公开方法
        行为：返回数组元素数量。
    impl Clone + PartialEq + Eq + Serialize + Deserialize for BlockInner

InnerType：字段类型，公开枚举
    Message--MESSAGE
    Tool--TOOL
    Sandbox--SANDBOX
    说明：删除旧ResourceID/ToolCall变体；空数组仍携带上述具体类型，不能按长度猜测。

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
    agent_components：Agent、AgentInfo、AgentState及WorldAgentExt
    lua_runtime_components：AgentLua、Driver通知与关闭槽位
    mcl_events：AgentMclQueue、MclPending、内部宿主提交及取消唤醒载荷、公开命令与结果
    execution_components：AgentExecutionScope及活动机制，Agent只引用一个作用域，不挂平行活动组件
    workspace_components：工作区身份、成员及WorldWorkspaceExt；workspace_execution提供线程入口与关闭能力
    resource_id_plugin：ResourceID、ResourceInner、ResourcesComponent、LibraryComponent、ResourceSet和基础查询
    types/resource_components：五类资源及AgentImage，AgentImage不强制有alias
    client_components：Client和其连接/发送器受检查询契约

资源规则：
    ResourceSet按完整ID存放目标Entity本地条目，可跨Entity有同ID，不能全局取第一个匹配者
    Agent内alias跨所有导入类型唯一，由ImportBindings及共享受检World入口统一维护；同alias不同完整ID先拒绝，Inference和MCL工具准备不能绕过检查
    IMPORT必须精确属于Agent镜像依赖，无builtin例外；MCL与加载插件共用WorldAgentExt.agent_validate_import受检预检，不维护平行许可或alias规则
    IMPORT TOOL tool:.../skill:.../hook:... AS alias产生CallableResource；IMPORT MESSAGE prompt:... AS alias按实际role产生AgentMessage（包括新System契约）；IMPORT SANDBOX sandbox:...产生SandboxValue并遵守同一alias检查
    重载成功更新资源条目及对应alias的新值，失败保留旧值；旧Block值持有不可变快照不暗变，激活沙箱改变策略时遵守Sandbox活动互斥

值与输出：
    GET alias或selector lists始终返回MESSAGE/TOOL/SANDBOX typed数组；列表按顺序只拼接同类型，混合类型在空数组情况下也拒绝
    INJECT左侧与GET同一求值规则，右侧为唯一写入范围；handler不按目标字段类型转换输入值
    tool_call effect第一参数必须为恰好一个Assistant的MESSAGE数组，第二参数必须为TOOL数组；只执行该Assistant.tool_calls
    空tool_calls返回MESSAGE空数组；重复调用id、多个Assistant或工具alias冲突均在提交任何执行前失败，不产生部分submit
    输入Assistant不在tool_call effect中重复记录，输出为ToolCall结果消息组成的MESSAGE数组
    history_append允许范围独立保持inject/user/assistant/tool_call/error，System不通过此effect追加；infer支持System
    inference完整返回InferenceResponse，保留stop_reason，不只抽取assistant正文
    MclCommandResult.result是真实执行结果；message保存仅由显式history_append发起，history_error只用于附带指令审计错误，不能因此重做工具
```

# error

## 逻辑

```text
错误归对应共享领域契约，不引用业务插件私有类型。
执行前历史失败阻止执行，执行后历史失败不覆盖真实结果；history_append本身失败正常回错误。
Cancelled、CleanupFailed、重复请求、alias冲突和幂等历史内容冲突必须有明确分类。
From<AsyncTaskError>不能凭空取得请求ID，闭包提交包装必须保存关联，不能仅日志后丢失回执。
```
