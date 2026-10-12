# 伪代码格式

```text
模块：使用一级标题，只写当前设计涉及的部分
类型：使用二级标题，按私有、crate公开和公开分组
TypeName：中文类型名，可见性类型--类型说明
    field_name: RustType--中文字段名，字段说明
    method_name<Generic>(self, parameter: ParameterType) -> ReturnType
        中文方法名：可见性方法，解释参数和用途
        约束：使用标准Rust where约束
        行为：展开完整逻辑
    impl TraitName for TypeName
        TraitName：可见性trait实现
        trait_method_name(&self, parameter: ParameterType) -> ReturnType
            中文方法名：解释参数和用途
            行为：标准库trait的简单行为用一句话说明
函数：使用二级标题，按私有和公开分组，只放置不属于某个类型的操作
function_name<Generic>(parameter: ParameterType) -> ReturnType
    中文函数名：可见性函数，解释参数和用途
    约束：使用标准Rust where约束
    行为：多步逻辑只使用一个行为块，在其下缩进展开
逻辑：使用二级标题，按执行顺序描述对象之间的调用关系
字段说明使用--，类型、方法和函数的说明直接写在标题中
属性：不写Rust Attribute，实现时自行判断
边界：对外使用泛型和具体类型，内部使用类型擦除
```

# 板块格式

本文描述当前Rust代码，不描述尚未实现的ResourceID、ResourceSet、共享组件文件或独立资源插件目标设计。
当前使用ResourceRef值、独立ResourceId组件、Agent.resources及resource_library_plugin访问接口。

迁移约定（尚未实现）：所有公开和内部事件将定义在共享types的tool_events.rs，插件不再保留events.rs。自动Result事件所需的具名载荷契约也归共享层。下文仍准确记录当前源码位置，不将未来迁移写成已经完成。

```text
lib        src/lib.rs                    Plugin、安装Resource、公开导出与函数
system     src/system.rs及handler内System  当前实际调度入口
handler    src/handler/*.rs               业务函数、读取、执行、进程处理
events     src/events.rs及异步通道         当前请求、结果和公开事件引用
types      src/types.rs、handler、bin      辅助数据结构与内部载荷
error      src/error.rs及handler/lua.rs    错误分类和真实映射
```

handler是目录：mod、skill、hook、lua、prompt、pty、sandbox七个文件。bin/tool_runner.rs是独立可执行目标，
在相应板块下单列。Component/Resource定义放lib说明；其余辅助类型在types说明。
声明为pub(crate)的函数仅本crate可用，pub(super)的包定位函数只对handler父模块可用。
handler模块本身私有；lua.rs中的pub类型不意味着从crate根可访问，只有显式重导出的项是公开入口。

# lib

## 类型

公开，src/lib.rs：
```text
ToolPlugin：工具插件，公开结构体--同时承担注册路由、资源包读取与调用分派
    root: Option<PathBuf>--请求的全局库根，私有
    schedule: String--运行计划，私有
    lua_limits: handler::lua::LuaExecutionLimits--执行限额，私有；没有公开setter
    open(root: impl Into<PathBuf>) -> Result<Self, ToolError>
        构造插件：公开关联函数
        行为：要求root绝对且不含ParentDir，失败返回InvalidRequest；保存Some(root)、UPDATE计划和默认限额，不创建目录
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置计划：公开方法，保存计划名并返回自身
    impl Default for ToolPlugin
        default() -> Self
            默认构造：root=None，计划为RuntimePlugin::UPDATE，限额为默认值
    impl Plugin for ToolPlugin
        build(self, app: &mut App)
            安装插件：公开trait方法
            行为：
                检查RuntimeHandle存在、ToolPluginInstalled不存在、指定计划存在；否则以对应ToolError panic
                读取已存在的ResourceLibraryRoot；显式root与已有root不同则assert失败
                按显式root、已有root、HOME/.margatroid的优先顺序选择根
                没有可用root且HOME缺失时panic；根不是绝对路径或含ParentDir也panic
                ResourceLibraryRoot缺失时插入；已存在时保留原Resource
                插入ToolPluginInstalled与LuaExecutionLimits
                依次安装tool_register_system、skill_register_system、hook_register_system、sandbox_register_system、prompt_register_system、lua_tool_register_system
                再安装tool_call_route_system、异步execute_prepared_lua_tool、lua_task_result_system、cancel_tool_turn_system
                build没有独立的AsyncRuntime安装检查，但add_async_system需要AsyncRegistry，缺失时由异步设施panic
                不安装额外消息清理System，不插入SkillRoots、HookRoots、LuaRoots或ShellRoots

ToolPluginInstalled：插件安装标记，公开Resource
    impl Resource for ToolPluginInstalled
```

公开，定义于src/handler/lua.rs并由lib重导出：
```text
LuaExecutionLimits：Lua限额，公开Resource--全部字段公开，可直接构造，因此不保证调用过new校验
    max_definition_bytes: usize--注册时单个metadata/schema文件上限
    max_script_bytes: usize--准备调用时脚本长度上限
    max_argument_bytes: usize--准备调用时参数文本上限
    max_output_bytes: usize--Lua字符串结果和进程输出保留上限
    max_memory_bytes: usize--mlua内存上限
    max_instructions: u64--指令预算
    max_execution_time: Duration--总执行及默认shell时间预算
    max_host_call_time: Duration--构造校验和载荷中保留的宿主时间配置，目前无执行消费者
    new(max_definition_bytes: usize, max_script_bytes: usize, max_argument_bytes: usize, max_output_bytes: usize, max_memory_bytes: usize, max_instructions: u64, max_execution_time: Duration, max_host_call_time: Duration) -> Result<Self, LuaError>
        构造限额：公开关联函数
        行为：任一字节数为0、指令数为0、任一时长为0或宿主时长大于执行时长时返回InvalidLimits，否则保存字段
    impl Default for LuaExecutionLimits
        default() -> Self
            默认限额：依次为64KiB、4MiB、1MiB、16MiB、256MiB、100000000指令、15分钟、5分钟，通过new构造
    impl Resource for LuaExecutionLimits
    impl Clone + Debug + PartialEq + Eq for LuaExecutionLimits
```

src/handler/sandbox.rs中的ConfinedSpawn、CallTempDir和ProcessGroup是crate公开辅助结构体，不实现Resource或Component，见types板块。
本crate不定义Agent或五类资源图书馆组件；它读取外部Agent、ResourceId、ResourceLibraryRoot与资源组件。
资源内容安装由其他消费注册结果的领域完成，注册handler本身只产出候选条目。

## 公开导出

```text
src/error.rs：ToolError、ToolErrorKind
src/events.rs：CancelToolTurn、ToolCallEvent、ToolRegisterRequest、ToolRegisterResponse
src/types.rs：ResourceContent、ResourceMapEntry、ToolCallRequest、ToolTemplate
src/handler/lua.rs：run_lua_tool、LuaExecutionLimits、LuaToolRunRequest
ToolExecutor使用agent_plugin的公开类型，本crate不定义或重导出它
LuaError与LuaErrorKind虽在lua.rs声明pub，但未重导出，不能通过tool_plugin::LuaError命名
```

## 函数

公开，src/lib.rs：
```text
register_agent_resource(world: &mut World, agent: Entity, entry: ResourceMapEntry) -> Result<ResourceMapEntry, ToolError>
    登记Agent工具引用：公开函数
    行为：
        executor为空返回InvalidDefinition，Agent组件缺失返回AgentNotAlive
        构造AgentResourceEntry，只复制resource_id、resource_name和executor
        调用Agent.resources.register_tool，错误映射DuplicateResource，成功原样返回entry
        不复制description或schema，不校验template存在，不在此安装资源组件

candidate_resource_entry(resource_id: ResourceRef, alias: Option<String>, executor: ToolExecutor, template: ToolTemplate) -> Result<ResourceMapEntry, ToolError>
    构造候选：公开函数，调用带内容版本，content=None

candidate_resource_entry_with_content(resource_id: ResourceRef, alias: Option<String>, executor: ToolExecutor, template: ToolTemplate, content: Option<ResourceContent>) -> Result<ResourceMapEntry, ToolError>
    构造带载荷候选：公开函数
    行为：
        resource_name取alias，否则取完整地址字符串；去空白后为空则InvalidDefinition
        覆盖template.name为resource_name并调用validate_template
        返回executor和template均为Some的ResourceMapEntry，不验证content与ID类型一致

resolve_agent_tool_definitions(world: &World, agent: Entity, resources: &[ResourceRef]) -> Result<Vec<ToolDefinition>, ToolError>
    解析模型工具清单：公开函数
    行为：
        resources为空直接返回空Vec，不先检查Agent
        非空时要求Agent存在；对每个地址要求Agent.resources.tools_by_resource恰好返回一条，否则ResourceResolutionFailed
        经resource_library_plugin::tool_definition取得描述和schema，访问错误映射ResourceResolutionFailed
        name取Agent条目的resource_name，description与input_schema取资源库，不读取Agent副本
        按输入地址顺序返回ToolDefinition列表

validate_agent_tool_calls(world: &World, agent: Entity, calls: &[ToolCall]) -> Result<(), ToolError>
    检查调用名称：公开函数
    行为：要求Agent存在，逐条检查tool_by_name命中；未命中返回ResourceResolutionFailed，不校验参数schema或资源实体
```

# system

## 函数

crate公开，src/system.rs：
```text
tool_register_system(world: &mut World)
    注册总路由System：crate公开System
    处理事件：ToolRegisterRequest
    产出事件：ToolRegisterResponse
    行为：
        克隆本帧全部请求
        id为空立即发InvalidRequest；已知skill、hook、sandbox、tool、prompt类型跳过，由各handler独立读取
        未知类型发ProviderMissing

tool_call_route_system(world: &mut World)
    调用路由System：crate公开System
    处理事件：ToolCallEvent
    产出事件：MclEffectReply、AgentFailure、AsyncEventRequest<PreparedLuaToolCall>
    行为：
        克隆本帧全部ToolCallEvent
        检查turn_id和call.id非空、Agent Entity存活、arguments能解析为JSON对象
        从Agent.resources按tool_name查唯一实际使用条目，构造ToolCallRequest
        要求Agent组件存在，拒绝重复(agent, turn_id, tool_call_id) pending键，再插入AgentToolPending
        SkillLoader同步调用execute_skill_call并finish_tool_call
        Hook同步调用execute_hook_call并finish_tool_call
        LuaRuntime调用prepare_lua_call，准备失败交finish_tool_call
        路由校验或重复pending失败时直接发MclEffectReply的Err和AgentFailure，kind=Agent
        此错误分支的效果回复id实际取event.turn_id，而非event.effect_id；这是当前实现差异，不在文档中修正代码

cancel_tool_turn_system(world: &mut World)
    取消记账System：crate公开System
    处理事件：CancelToolTurn
    产出事件：无
    行为：存在Agent时移除属于目标Agent和turn_id的tools.pending条目；不取消异步任务或子进程，不通知Lua guard
```

crate公开，定义在handler各文件，实际仍是安装的System：
```text
skill_register_system(world: &mut World)
    技能注册System：crate公开System，src/handler/skill.rs
    处理事件：ToolRegisterRequest--只处理skill类型
    产出事件：ToolRegisterResponse
    行为：读取请求并执行skill注册读取流程，逐条发送保留id、agent、resource_id、alias的候选或错误

hook_register_system(world: &mut World)
    钩子注册System：crate公开System，src/handler/hook.rs
    处理事件：ToolRegisterRequest--只处理hook类型
    产出事件：ToolRegisterResponse
    行为：读取请求并执行hook注册读取流程，逐条发送候选或错误

sandbox_register_system(world: &mut World)
    沙箱注册System：crate公开System，src/handler/sandbox.rs
    处理事件：ToolRegisterRequest--只处理sandbox类型
    产出事件：ToolRegisterResponse
    行为：逐条调用register_sandbox_resource，失败记录error日志，然后发送候选或错误

prompt_register_system(world: &mut World)
    提示注册System：crate公开System，src/handler/prompt.rs
    处理事件：ToolRegisterRequest--只处理prompt类型
    产出事件：ToolRegisterResponse
    行为：逐条调用register_prompt_resource并发送候选或错误

lua_tool_register_system(world: &mut World)
    Lua工具注册System：crate公开System，src/handler/lua.rs
    处理事件：ToolRegisterRequest--只处理tool类型
    产出事件：ToolRegisterResponse
    行为：逐条调用register_lua_tool并发送候选或错误

execute_prepared_lua_tool(prepared: PreparedLuaToolCall) -> Result<(), LuaTaskError>
    Lua执行异步Handler：crate公开异步函数，src/handler/lua.rs，通过add_async_system安装
    处理事件：AsyncEventRequest<PreparedLuaToolCall>--由通用异步System读取包装后传入拥有所有权的prepared
    产出事件：Result<(), LuaTaskError>--自动Pending响应升变；MclEffectReply及可能的AgentFailure由回执guard发送
    行为：以max_execution_time包装execute_lua_tool，超时得到ExecutionFailed，再调用guard.respond，正常结束返回Ok(())

lua_task_result_system(world: &mut World)
    异步监督结果System：crate公开System，src/handler/lua.rs
    处理事件：Result<(), LuaTaskError>
    产出事件：无
    行为：只对Err记录warn，不清pending、不生成业务回复；未完成任务的业务兜底由guard Drop负责
```

## 逻辑

```text
多个注册System读取的是同一事件流，读取不独占消费
空id请求可被总路由与具体注册System各回复一次，不能宣称每条注册请求恰好一个响应
skill和hook具体注册流程不检查id是否为空；lua、sandbox和prompt自身也检查空id
各注册流程同步读磁盘，不是新资源设计中的异步加载闭包
异步执行请求、自动Result<(), LuaTaskError>和公开MclEffectReply是不同事件通道
取消System只删除记账：同步finish发现pending不存在就不回复，Lua guard不检查pending仍可能回复
Lua回执的pending清理由外部MCL接受Tool回复后处理；mcl_plugin/src/system.rs的mcl_effect_response_system按Agent和tool_call_id移除记录
```

# handler

## src/handler/mod.rs：公共根与同步回执

crate公开：
```text
library_root(world: &World) -> Result<&Path, ToolError>
    读取全局根：crate公开函数，缺ResourceLibraryRoot返回ToolEnvironmentMissing

finish_tool_call(world: &mut World, request: ToolCallRequest, result: Result<String, ToolError>)
    完成同步调用：crate公开函数
    行为：
        移除Agent.tools.pending中的(agent, turn_id, tool_call_id)，没有记录则返回不发送结果
        result为Err时把错误文本作为content并failed=true，为Ok时failed=false
        发送MclEffectReply，id=request.effect_id，result=Ok(Message::Tool)，usage=None
        不解析JSON结果信封，不根据错误kind发送AgentFailure
```

## src/handler/skill.rs：技能

crate公开：
```text
execute_skill_call(world: &World, request: &ToolCallRequest) -> Result<String, ToolError>
    执行技能读取：crate公开函数
    行为：
        要求Agent存在、地址类型skill且tag=latest
        经资源库read_skill_content取得文本，错误映射ResourceResolutionFailed
        验证arguments为JSON对象，重新解析SKILL.md的TOML前置信息并返回trim后的正文
```

父模块可见：
```text
find_skill_file(library_root: &Path, resource: &ResourceRef) -> Result<PathBuf, ToolError>
    定位技能文件：pub(super)函数，检查全局skills/scope/name/tag目录存在且为目录、SKILL.md为文件，返回文件路径
```

私有：
```text
parse_skill_document(source: &str) -> Result<SkillDocument, ToolError>
    解析技能：私有函数，要求+++换行起始和换行+++换行终止，TOML name/description非空、正文非空；不比较name与ID
validate_skill_resource(resource: &ResourceRef) -> Result<(), ToolError>
    校验技能地址：私有函数，只允许skill与latest
```

技能注册流程先要求Agent存在，再校验地址并直接read_to_string，解析文档后构造schema={"type":"object"}、
executor=SkillLoader、content=Skill{content_path}的候选。该读取没有文件字节上限；执行阶段也重新解析前置TOML。

## src/handler/hook.rs：钩子

crate公开：
```text
execute_hook_call(_world: &World, _request: &ToolCallRequest) -> Result<String, ToolError>
    执行钩子：crate公开函数，实际无校验、无后端行为，返回Ok空字符串
```

父模块可见：
```text
find_hook_package(library_root: &Path, resource: &ResourceRef) -> Result<PathBuf, ToolError>
    定位钩子包：pub(super)函数，检查全局hooks/scope/name/tag目录、hook.toml和input.schema.json
```

私有：
```text
read_hook_metadata(package_root: &Path) -> Result<HookMetadata, ToolError>
    读取元数据：私有函数，同步读TOML，未知字段拒绝，name和description非空；不比较name与资源ID
read_hook_schema(package_root: &Path) -> Result<Value, ToolError>
    读取schema：私有函数，同步读并解析JSON；本函数不构建JSON Schema验证器
validate_hook_resource(resource: &ResourceRef) -> Result<(), ToolError>
    校验钩子地址：私有函数，只允许hook与latest
```

钩子注册先要求Agent组件存在，读取元数据/schema，ToolTemplate只进一步要求schema是对象；候选executor=Hook、
content=Hook。加载文件无字节上限。Hook本质是driver解释的触发器，本crate执行函数不产生触发行为。

## src/handler/prompt.rs：提示

私有：
```text
register_prompt_resource(world: &World, request: &ToolRegisterRequest) -> Result<ResourceMapEntry, ToolError>
    注册提示：私有函数
    行为：
        空id或Entity不存活返回InvalidRequest，不另检查Agent组件
        定位包、读取role与正文，resource_name取alias或完整ID
        返回executor=None、template=None、content=Prompt{role, content: Arc<str>}的条目
read_prompt_metadata(package: &Path) -> Result<PromptMetadata, ToolError>
    读取提示元数据：私有函数，同步读取prompt.toml，拒绝未知字段，role只允许system/user/assistant
read_prompt_content(package: &Path) -> Result<String, ToolError>
    读取提示正文：私有函数，同步读取content.md，trim为空则InvalidDefinition；返回原文本，不限大小
```

父模块可见：
```text
find_prompt_package(library_root: &Path, resource_id: &ResourceRef) -> Result<PathBuf, ToolError>
    定位提示目录：pub(super)函数，要求prompt与latest，检查全局prompts/scope/name/tag为目录；文件存在性由读取验证
```

## src/handler/lua.rs：注册、执行和runner

crate公开入口prepare_lua_call及私有准备：
```text
prepare_lua_call(world: &mut World, request: ToolCallRequest) -> Result<(), ToolError>
    准备异步调用：crate公开函数
    行为：调用prepare_lua_tool_call取得执行快照和临时目录，创建LuaToolResponseGuard，send_async_event(PreparedLuaToolCall)，不直接返回工具结果

register_lua_tool(world: &mut World, request: &ToolRegisterRequest) -> Result<ResourceMapEntry, ToolError>
    注册Lua候选：私有函数
    行为：
        空id或Agent Entity不存活返回InvalidRequest，要求Agent组件和已安装限额
        在全局tools/scope/name/tag查找三文件齐备的目录
        有界读取tool.toml和input.schema.json，调用parse_lua_tool_definition
        构造executor=LuaRuntime、ToolTemplate和content=Tool{script_path}候选；注册时不读脚本文本

prepare_lua_tool_call(world: &World, request: &ToolCallRequest) -> Result<(LuaExecutionHandle, CallTempDir), ToolError>
    准备执行材料：私有函数
    行为：
        要求非空turn_id/tool_call_id、资源类型tool、参数长度未超上限、Agent存活
        读取Agent及其ResourceId组件，缺失返回InvalidRequest
        读取资源库已解析描述/schema和脚本，取得激活沙箱策略的本次快照
        在Agent项目根创建CallTempDir，组装上下文和执行handle

read_lua_tool_package(world: &World, resource_id: &ResourceRef, limits: &LuaExecutionLimits) -> Result<LuaToolPackage, ToolError>
    读取执行包：私有函数
    行为：经tool_resource和read_tool_script读取组件定义与脚本，库错误映射ResourceResolutionFailed；脚本读完后检查长度及非空，不解析TOML

parse_lua_tool_definition(metadata_source: &str, schema_source: &str, resource_id: &ResourceRef) -> Result<LuaToolDefinition, ToolError>
    解析注册文件：私有函数
    行为：
        TOML反序列化LuaToolManifest，拒绝未知字段
        schema_version必须为1，name非空且等于resource_id.name，description非空
        JSON schema必须为对象且可构建验证器，返回已解析描述与parameters
```

父模块可见：
```text
find_lua_tool_package(library_root: &Path, resource_id: &ResourceRef) -> Result<PathBuf, ToolError>
    定位Lua包：pub(super)函数，要求类型tool，任意合法tag按路径查找；验证目录与tool.toml/input.schema.json/main.lua三个文件
```

公开导出：
```text
run_lua_tool(request: LuaToolRunRequest) -> Result<String, ToolError>
    执行内联Lua载荷：公开异步函数
    行为：
        使用request中已解析description/input_schema，不读磁盘包、不再解析TOML
        解析arguments为JSON对象，构建schema验证器并验证参数
        构造执行handle；创建unsafe StdLib::ALL Lua，设置内存限制，安装指令hook和margatroid环境
        执行script顶层，按资源ID设置chunk名称；取得全局execute函数，缺失返回InvalidDefinition
        此后才把arguments填入入口表，因此script顶层尚看不到已填充的入口arguments
        调用execute(arguments, context)，通过call_async::<String>接收字符串，不接受任意Lua对象作为结果
        最终字符串超过max_output_bytes返回ExecutionFailed，否则原样返回，不在本函数解包JSON信封
```

私有执行函数：
```text
execute_lua_tool(prepared: &PreparedLuaToolCall) -> Result<String, ToolError>
    执行准备载荷：私有异步函数，复制上下文、内容、限额和策略到LuaToolRunRequest，调用spawn_tool_runner

tool_runner_path() -> Result<PathBuf, ToolError>
    定位runner：私有函数，优先MARGATROID_TOOL_RUNNER，否则当前可执行文件同目录的tool_runner或Windows名称tool_runner.exe；不预先检查文件存在

runner_payload(request: &LuaToolRunRequest) -> Value
    编码runner请求：私有函数
    行为：写description、input_schema、script、目录、参数与地址字符串，限额时长转u64毫秒；不包含sandbox_policies

spawn_tool_runner(request: &LuaToolRunRequest) -> Result<String, ToolError>
    派生runner：私有异步函数
    行为：
        编码JSON，定位runner，用request.sandbox_policies建立ConfinedSpawn并构造命令
        stdin/stdout/stderr均为管道，pre_exec调用setsid，启动子进程；只有策略非空时启用ProcessGroup守卫
        写完stdin并shutdown；写入过程本函数没有内部timeout，外层异步执行timeout覆盖它
        取得输出管道，在内部max_execution_time超时块中先await child.wait，再reclaim进程组，再try_join读取两条输出管道
        内部等进程、读取失败或超时返回RunnerFailed；超时时reclaim
        成功退出且stdout截断返回ExecutionFailed；未截断返回UTF-8有损转换的stdout，成功路径不使用stderr内容
        非零退出经runner_failure解析stderr

runner_failure(stderr: &str) -> ToolError
    还原runner错误：私有函数
    行为：
        从最后一行向前找可解析JSON且kind属于六种可识别分类的行，忽略其他trap日志
        只保留InvalidRequest、InvalidArguments、InvalidDefinition、Aborted
        ExecutionFailed、RunnerFailed及不可解析情况均映射ExecutionFailed，RunnerFailed会在此降级
        message缺失使用Lua tool execution failed

install_execution_hook(lua: &Lua, limits: &LuaExecutionLimits) -> Result<(), ToolError>
    安装指令检查：私有函数，每1000条指令累加预算并检查Instant截止时间，超过预算或时间返回mlua运行时错误；不监控等待宿主期间的每个时刻

install_lua_environment<'lua>(lua: &'lua Lua, handle: &LuaExecutionHandle) -> Result<Table, ToolError>
    安装工具环境：私有函数
    行为：
        建立context表，含agent_id、turn_id、resource_id、project_root、temp_dir，并用read_only_proxy包裹
        建立入口表version=1、context、json.encode/json.decode、shell，保存在注册表margatroid_entry
        全局margatroid是取回入口表的函数，返回值是context代理供execute第二参数使用
        此时不填arguments；始终安装shell，与是否导入shell资源无关

install_shell(lua: &Lua, entry: &Table, handle: &LuaExecutionHandle) -> Result<(), ToolError>
    安装命令入口：私有函数
    行为：
        注入异步shell(command: String, options: Option<Table>)，空白命令报mlua运行时错误
        options.max_output_bytes和timeout_ms可覆盖默认值，不钳制到总限额，也不使用max_host_call_time
        总是ConfinedSpawn::new(&[])，调用run_in_pty以bash -c执行命令
        返回exit_code或nil、合并stdout文本、truncated和terminal；非零退出码本身不由此函数变成ToolError

read_only_proxy(lua: &Lua, values: Table) -> Result<Table, ToolError>
    构造context代理：私有函数，用__index读取底表，__newindex报错且隐藏metatable；不是冻结完整Lua环境，StdLib::ALL仍可提供其他修改能力

ends_the_turn(kind: ToolErrorKind) -> bool
    判定Lua guard终止分支：私有函数，仅Aborted或RunnerFailed返回true，不能用于概括其他路由和同步回执路径

reported_outcome(output: &str) -> Option<(bool, String)>
    读取结果信封：私有函数，要求JSON对象有bool型ok和string型content；允许额外字段与空content，其他字符串返回None

read_process_output(reader: impl AsyncRead + Unpin, limit: usize) -> std::io::Result<ProcessOutputBuffer>
    收集进程流：私有异步函数，每次读8192字节，超限只停止保留但继续排读到EOF，读错误向上返回

read_bounded_sync(path: &Path, limit: usize, label: &str) -> Result<String, ToolError>
    有界读取定义：私有函数，用take(limit+1)读取，文件和读取失败返回ResourceResolutionFailed，调用decode_bounded

decode_bounded(bytes: Vec<u8>, limit: usize, label: &str) -> Result<String, ToolError>
    校验定义文本：私有函数，超限或非UTF-8返回InvalidDefinition

lua_tool_error(error: mlua::Error) -> ToolError
    映射Lua错误：私有函数，直接RuntimeError变为Aborted，其余变ExecutionFailed；不检查错误是否由脚本主动调用error，不保证所有限额错误的外层变体相同
```

## src/handler/sandbox.rs：策略与进程设施

crate公开：
```text
active_sandbox_policies(world: &World, resources: &AgentResourceMap) -> Result<Vec<Arc<str>>, ToolError>
    获取策略快照：crate公开函数，按active_sandboxes集合序从资源库读取策略，不可用或trim为空返回ExecutionFailed，不读取Agent私有策略副本
sandbox_backend() -> Option<PathBuf>
    定位后端：crate公开函数，优先MARGATROID_SANDBOX_BACKEND，再PATH中的landstrip，再当前可执行文件同目录；环境覆盖不预检文件
```

父模块可见：
```text
find_sandbox_package(library_root: &Path, resource_id: &ResourceRef) -> Result<PathBuf, ToolError>
    定位策略：pub(super)函数，要求sandbox类型，任意tag在全局sandboxes/scope/name/tag中寻找policy.json文件
```

私有：
```text
register_sandbox_resource(world: &World, request: &ToolRegisterRequest) -> Result<ResourceMapEntry, ToolError>
    注册策略：私有函数
    行为：
        空id或Entity失效返回InvalidRequest，要求Agent组件
        同步read_to_string(policy.json)，读失败返回InvalidDefinition
        空字符串或超过MAX_POLICY_BYTES=1MiB拒绝，JSON可解析即可，不要求对象或特定后端schema
        返回executor=None、template=None和Sandbox{policy}，resource_name取alias或完整ID，不经validate_template
write_policy_file(policy: &str) -> Result<PathBuf, ToolError>
    暂存策略：私有函数，在系统临时目录写margatroid-sandbox-policy-{pid}-{counter}.json，写失败RunnerFailed
remove_policy_files(policies: &[PathBuf])
    删除策略：私有函数，逐个删除，错误忽略
inherited_environment() -> Vec<(OsString, OsString)>
    继承环境：私有函数，过滤凭据形名字后的vars_os
is_withheld_env_name(name: &str) -> bool
    检查凭据名：私有函数，ASCII大写后包含KEY/PASSWORD/SECRET/TOKEN任一子串则过滤
inherit_environment(command: &mut tokio::process::Command)
    设置进程环境：私有函数，清空后写回过滤列表，再固定NO_COLOR=1、TERM=dumb、PAGER=cat、GIT_PAGER=cat
```

## src/handler/pty.rs：命令进程

crate公开：
```text
run_in_pty(request: PtyRequest<'_>, spawn: &ConfinedSpawn) -> Result<PtyOutcome, ToolError>
    执行命令：crate公开异步函数
    行为：
        spawn_command取得child与流，ProcessGroup只有spawn.is_confined时启用
        构造collect Future但不立刻poll它，select等待child退出或超时
        超时reclaim并返回ExecutionFailed，正常退出reclaim后才await collect
        返回exit_code、bytes、truncated和terminal；wait错误也ExecutionFailed
normalize_pty_text(bytes: &[u8]) -> String
    规范输出：crate公开函数，UTF-8有损转换并把CRLF替换为LF
```

私有：
```text
collect(streams: Streams, limit: usize) -> (Vec<u8>, bool)
    收集输出：私有异步函数，Terminal读master，EIO视正常结束；Pipes在本函数被poll后join读取stdout/stderr，先stdout后追加stderr，不保留跨流时序
drain(reader: impl AsyncRead + Unpin, limit: usize, output: &mut Vec<u8>, truncated: &mut bool)
    排读管道：私有异步函数，8192字节缓冲，Interrupted重试，其他错误直接结束不返回错误
spawn_command(request: PtyRequest<'_>, spawn: &ConfinedSpawn) -> Result<(tokio::process::Child, Streams), ToolError>
    构造命令：私有函数
    行为：
        open_terminal成功时用slave作为三个标准流，program -c command，TERM=xterm-256color
        PTY分支pre_exec执行setsid和TIOCSCTTY，建立新的会话与控制终端
        open_terminal失败时退回null stdin和两条piped输出，不设置新的session
        两分支都经过ConfinedSpawn::command和kill_on_drop
start(process: tokio::process::Command) -> Result<tokio::process::Child, ToolError>
    启动子进程：私有函数，spawn失败映射ExecutionFailed
open_terminal() -> Option<(tokio::fs::File, Stdio, Stdio, Stdio, RawFd)>
    尝试终端：私有函数，openpty或slave克隆失败返回None，否则返回master和slave三标准流
extend_bounded(output: &mut Vec<u8>, chunk: &[u8], limit: usize, truncated: &mut bool)
    保留有界字节：私有函数，最多追加剩余容量，有丢弃则标记truncated
```

## src/bin/tool_runner.rs：独立程序

全部函数bin私有：
```text
kind_name(kind: ToolErrorKind) -> &'static str
    编码分类：私有函数，保留InvalidRequest/InvalidArguments/InvalidDefinition/RunnerFailed/Aborted，其余输出ExecutionFailed
report(error: &ToolError) -> ExitCode
    报告错误：私有函数，stderr输出kind与message JSON，三个Invalid分类返回2，其余返回3
request_payload() -> Result<String, String>
    读取请求：私有函数，有argv[1]时同步读文件，否则从stdin读到EOF，不自带字节上限
main() -> ExitCode
    runner入口：私有异步函数，Tokio current_thread运行
    行为：
        请求读取、JSON解码或地址解析失败时直接stderr输出RunnerFailed并返回2，不经report
        将RunnerLimits毫秒值转Duration，直接构造LuaExecutionLimits，不调用new检查
        将字符串地址解析成ResourceRef，构造sandbox_policies为空的LuaToolRunRequest并调用run_lua_tool
        成功原样write_all字符串到stdout，忽略stdout写入错误并返回0；失败调用report
```

## 当前执行边界与限制

```text
原始文件与执行快照：
    注册Lua工具时解析原始tool.toml和JSON文件；执行Lua时description/input_schema已经是解析值
    run_lua_tool使用内联script，不读metadata或脚本路径；准备阶段从资源库取得脚本快照
    read_lua_tool_package先读完整脚本再检查长度，不能宣称其读取过程本身有界
    技能执行仍解析SKILL.md的TOML前置信息，不能概括全部执行路径都不解析TOML
    包查找使用fs::metadata/is_file，未全面拒绝symlink，不承诺路径链不可替换

沙箱范围：
    常规异步插件路径在外部spawn_tool_runner时以策略包裹runner，策略不通过JSON传入runner
    runner内shell用空策略ConfinedSpawn，依靠所在进程已经获得的外层约束，不二次包裹
    直接公开调用run_lua_tool在当前进程创建StdLib::ALL VM，传入sandbox_policies不会自动隔离该VM或shell
    shell始终安装，无shell资源导入要求，当前不存在独立ShellError或Shell工具资源

输出与超时：
    PTY先wait再poll collect，runner也是先wait再排读；大输出可能填满缓冲使子进程无法退出，最终超时
    collect内部并发读两条管道不等于等待child期间也在读取，不能宣称已完整防阻塞
    PTY超时仅覆盖wait，不覆盖其后的collect；后代保持流打开时收集阶段可能继续等待
    runner内部timeout包含wait与后续排读，但不包含此前stdin投递；外层execute_prepared_lua_tool还包住完整执行
    外层超时映射ExecutionFailed，内部runner超时映射RunnerFailed，两者不能统一描述成终止轮次

进程组：
    ProcessGroup仅策略非空才记录leader，killpg错误忽略，它不是任意进程树遍历
    runner总是setsid，但无策略时ProcessGroup不启用
    注入shell传空策略，因此它在run_in_pty里的ProcessGroup守卫不启用；PTY子进程又创建独立session
    外层runner进程组回收不保证覆盖另建session的shell或其后代，不能宣称所有命令后台进程必定清理
    kill_on_drop只处理直接子进程；pipe fallback也没有新的session设置
    两种spawn都继承NO_COLOR和pager环境；PTY仅覆盖TERM，未清除其他固定值
```

# events

## 类型

公开，src/events.rs：
```text
ToolRegisterRequest：注册请求，公开事件
    id: String--请求ID
    agent: Entity--目标Agent
    resource_id: ResourceRef--资源地址
    alias: Option<String>--本地别名
    impl Clone + Debug + PartialEq + Eq for ToolRegisterRequest
    impl Event for ToolRegisterRequest
ToolRegisterResponse：注册结果，公开事件
    id: String--原请求ID
    agent: Entity--原Agent
    resource_id: ResourceRef--原资源地址
    alias: Option<String>--原别名
    result: Result<ResourceMapEntry, ToolError>--候选或错误
    impl Clone + Debug + PartialEq for ToolRegisterResponse
    impl Event for ToolRegisterResponse
CancelToolTurn：取消记账请求，公开事件
    turn_id: String--轮次ID
    agent: Entity--目标Agent
    impl Clone + Debug + PartialEq + Eq for CancelToolTurn
    impl Event for CancelToolTurn
ToolCallEvent：共享margatroid_types事件，公开重导出
    effect_id: String--效果ID
    turn_id: String--轮次ID
    agent: Entity--目标Agent
    call: ToolCall--调用ID、名称、参数
```

crate公开，src/handler/lua.rs：
```text
PreparedLuaToolCall：异步输入，crate公开事件
    arguments: String--参数，私有
    handle: LuaExecutionHandle--快照，私有
    response: LuaToolResponseGuard--业务回执，私有
    _temp_dir: CallTempDir--临时目录守卫，私有
    impl Event for PreparedLuaToolCall
```

## 逻辑

```text
send_async_event把PreparedLuaToolCall包成AsyncEventRequest<PreparedLuaToolCall>
AsyncRuntime创建Pending<Result<(), LuaTaskError>>，完成后升变为普通Result事件
业务回复不在这个自动Result中，由guard发送MclEffectReply
正常Tool回复含resource_id、tool_call_id、content、failed，usage=None，效果回复关联effect_id
Lua终止分支另外发AgentFailure，关联turn_id且kind=Agent
路由校验失败也发上述两种事件，但效果回复目前关联turn_id
本crate不发送AgentMessage，接收回执和推进Driver由外部MCL领域负责
```

# types

## src/types.rs：公开值类型

```text
ToolTemplate：模型工具候选，公开结构体
    name: String
    description: String
    parameters: serde_json::Value
    new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Result<Self, ToolError>
        构造模板：公开方法，调用validate_template；不检查name非空、不构建schema验证器
    impl Clone + Debug + PartialEq for ToolTemplate
ResourceContent：注册载荷，公开枚举
    Tool { script_path: PathBuf }
    Skill { content_path: PathBuf }
    Hook
    Prompt { role: String, content: Arc<str> }
    Sandbox { policy: Arc<str> }
    impl Clone + Debug + PartialEq for ResourceContent
ResourceMapEntry：注册中转条目，公开结构体
    resource_id: ResourceRef
    resource_name: String
    alias: Option<String>
    executor: Option<ToolExecutor>
    template: Option<ToolTemplate>
    content: Option<ResourceContent>
    impl Clone + Debug + PartialEq for ResourceMapEntry
ToolCallRequest：内部已解析调用，公开结构体--不实现Event
    effect_id: String
    turn_id: String
    agent: Entity
    executor: ToolExecutor
    resource_id: ResourceRef
    tool_call_id: String
    arguments: String
    impl Clone + Debug + PartialEq + Eq for ToolCallRequest
```

crate公开函数，src/types.rs：
```text
validate_template(template: &ToolTemplate) -> Result<(), ToolError>
    校验模板：crate公开函数，description.trim为空或parameters不是对象返回InvalidDefinition
```

## 常量与静态变量

```text
src/handler/skill.rs：PROVIDER_ID="skill"、SKILL_FILE="SKILL.md"
src/handler/hook.rs：PROVIDER_ID="hook"、HOOK_FILE="hook.toml"、HOOK_SCHEMA_FILE="input.schema.json"
src/handler/prompt.rs：PROMPT_TYPE="prompt"、PROMPT_FILE="content.md"、PROMPT_METADATA_FILE="prompt.toml"
src/handler/lua.rs：TOOL_METADATA_FILE="tool.toml"、TOOL_SCHEMA_FILE="input.schema.json"、TOOL_SCRIPT_FILE="main.lua"
src/handler/sandbox.rs：SANDBOX_TYPE="sandbox"、POLICY_FILE="policy.json"、MAX_POLICY_BYTES=1048576
    POLICY_FILE_COUNTER: AtomicU64--私有静态，初始0，临时策略文件和调用目录共用，Relaxed递增
    WITHHELD_ENV_PATTERN: [&str; 4]--私有常量，KEY、PASSWORD、SECRET、TOKEN
```

## src/handler/lua.rs：执行载荷

公开重导出：
```text
LuaToolRunRequest：内联执行请求，公开结构体--不实现Event、未派生Serialize或Deserialize
    description: String
    input_schema: serde_json::Value
    script: String
    temp_dir: PathBuf
    arguments: String
    agent_id: ResourceRef
    turn_id: String
    resource_id: ResourceRef
    project_root: PathBuf
    limits: LuaExecutionLimits
    sandbox_policies: Vec<Arc<str>>
```

私有：
```text
LuaToolManifest：注册TOML对象，私有结构体--Deserialize拒绝未知字段
    schema_version: u32
    name: String
    description: String
    impl Clone + Debug + PartialEq + Eq for LuaToolManifest
LuaToolDefinition：已解析定义，私有结构体
    description: String
    parameters: serde_json::Value
LuaToolPackage：执行包快照，私有结构体
    description: String
    input_schema: serde_json::Value
    script: String
LuaToolCallLocator：回执关联，私有结构体
    effect_id: String
    turn_id: String
    agent: Entity
    tool_call_id: String
    resource_id: ResourceRef
LuaCallContext：调用上下文，私有结构体
    agent_id: ResourceRef
    turn_id: String
    resource_id: ResourceRef
    project_root: Arc<PathBuf>
    temp_dir: Arc<PathBuf>
    sandbox_policies: Vec<Arc<str>>
LuaExecutionHandle：执行快照，私有结构体
    context: LuaCallContext
    limits: LuaExecutionLimits
    package: LuaToolPackage
LuaToolResponseGuard：业务回执守卫，私有结构体
    locator: Option<LuaToolCallLocator>
    events: RuntimeEventSender
    new(request: &ToolCallRequest, events: RuntimeEventSender) -> Self
        构造守卫：私有关联函数，复制请求关联
    respond(&mut self, result: Result<String, ToolError>)
        发送结果：私有方法
        行为：
            take locator，已经发送则panic
            Ok字符串可解包信封时用content和!ok，否则保留原字符串并failed=false
            Err且ends_the_turn为true时发送MclEffectReply Err与AgentFailure，返回
            其他Err作为failed=true的Tool消息，正常结果均发送MclEffectReply Ok(Message::Tool)
            不访问World或删除pending
    impl Drop for LuaToolResponseGuard
        drop(&mut self)
            兜底回执：尚有locator时发送failed=true的Tool效果回复，错误为ExecutionFailed，不发AgentFailure
ProcessOutputBuffer：有界进程输出，私有结构体
    bytes: Vec<u8>
    truncated: bool
```

## src/handler/skill.rs、hook.rs、prompt.rs：解析类型

私有：
```text
SkillMetadata：技能TOML头，私有结构体--Deserialize拒绝未知字段
    name: String
    description: String
SkillDocument：技能解析结果，私有结构体
    metadata: SkillMetadata
    body: String
HookMetadata：钩子TOML头，私有结构体--Deserialize拒绝未知字段
    name: String
    description: String
PromptMetadata：提示TOML头，私有结构体--Deserialize拒绝未知字段
    role: String
```

## src/handler/sandbox.rs：进程和文件守卫

crate公开，全部字段私有：
```text
ConfinedSpawn：派生命令配置，crate公开结构体
    policies: Vec<PathBuf>--临时策略文件
    new(policies: &[Arc<str>]) -> Result<Self, ToolError>
        暂存策略：crate公开关联函数，逐个写临时文件，失败删除此前已写文件；无策略为非受限模式
    is_confined(&self) -> bool
        查询受限标志：crate公开方法，policies非空
    command(&self, program: &Path, project_root: &Path) -> Result<tokio::process::Command, ToolError>
        构造命令：crate公开方法
        行为：策略非空时寻找landstrip后端，缺失返回RunnerFailed；构造landstrip run -p各策略 -- program；否则直接program，统一cwd、kill_on_drop和过滤环境
    impl Drop for ConfinedSpawn
        drop(&mut self)
            删除策略文件：删除失败忽略
CallTempDir：调用临时目录，crate公开结构体
    path: PathBuf
    create(project_root: &Path) -> Result<Self, ToolError>
        创建目录：crate公开关联函数，以共享原子counter生成project_root/.margatroid/tmp/{counter}，create_dir_all失败ExecutionFailed
    path(&self) -> &Path
        读取路径：crate公开方法
    impl Drop for CallTempDir
        drop(&mut self)
            清理目录：remove_dir_all，错误忽略
ProcessGroup：可选进程组守卫，crate公开结构体
    leader: Option<u32>
    confined(confined: bool, leader: u32) -> Self
        构造守卫：crate公开关联函数，仅confined=true保留leader
    reclaim(&mut self)
        请求组终止：crate公开方法，take leader后向非0组ID发送SIGKILL，忽略错误，后续调用无操作
    impl Drop for ProcessGroup
        drop(&mut self)
            回收组：调用reclaim
```

## src/handler/pty.rs：终端载荷

crate公开，字段均为pub(crate)：
```text
PtyRequest<'a>：命令请求，crate公开结构体
    program: &'a Path
    command: &'a str
    project_root: &'a Path
    max_output_bytes: usize
    max_execution_time: Duration
    impl Clone + Copy for PtyRequest<'a>
PtyOutcome：命令结果，crate公开结构体
    exit_code: Option<i32>
    bytes: Vec<u8>
    truncated: bool
    terminal: bool
    impl Debug for PtyOutcome
```

私有：
```text
Streams：子进程输出路径，私有枚举
    Terminal(tokio::fs::File)
    Pipes { stdout: tokio::process::ChildStdout, stderr: tokio::process::ChildStderr }
```

## src/bin/tool_runner.rs：bin私有反序列化类型

```text
RunnerRequest：JSON请求，bin私有结构体
    description: String
    input_schema: serde_json::Value
    script: String
    temp_dir: PathBuf
    arguments: String
    agent_id: String
    turn_id: String
    resource_id: String
    project_root: PathBuf
    limits: RunnerLimits
    impl Deserialize for RunnerRequest
RunnerLimits：JSON限额，bin私有结构体
    max_definition_bytes: usize
    max_script_bytes: usize
    max_argument_bytes: usize
    max_output_bytes: usize
    max_memory_bytes: usize
    max_instructions: u64
    max_execution_time_ms: u64
    max_host_call_time_ms: u64
    impl Deserialize for RunnerLimits
```

地址和目录在JSON中序列化为字符串，目录采用to_string_lossy。策略字段故意不在RunnerRequest中，已在外部派生时应用。
反序列化类型没有deny_unknown_fields；runner直接构造限额而不走new校验。

# error

## src/error.rs：公开错误

```text
ToolErrorKind：工具错误分类，公开枚举
    AgentMissing
    ResourceMapMissing
    InvalidResource
    ResourceUnavailable
    RegistrationFailed
    ToolCallMissing
    InvalidDefinition
    ProviderMissing
    ResourceResolutionFailed
    AgentNotAlive
    ToolEnvironmentMissing
    ToolPluginMissing
    ToolAlreadyRegistered
    DuplicateResource
    InvalidRequest
    InvalidArguments
    ExecutionFailed
    RunnerFailed
    Aborted
    impl Clone + Copy + Debug + PartialEq + Eq for ToolErrorKind
ToolError：有界工具错误，公开结构体
    kind: ToolErrorKind--私有字段
    message: String--私有字段
    new(kind: ToolErrorKind, message: impl Into<String>) -> Self
        构造错误：公开关联函数，文本超过512字节时在509以内UTF-8边界截断并加三个点
    kind(&self) -> ToolErrorKind
        读取分类：公开方法
    message(&self) -> &str
        读取描述：公开方法
    panic(self) -> !
        终止：crate公开方法，以Display文本panic
    impl Clone + Debug + PartialEq + Eq for ToolError
    impl fmt::Display for ToolError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化错误：输出Debug分类、冒号、消息
    impl std::error::Error for ToolError
```

## src/handler/lua.rs：内部路径错误

```text
LuaErrorKind：限额构造错误，lua模块中pub枚举--不从crate根重导出
    InvalidLimits
    impl Clone + Copy + Debug + PartialEq + Eq for LuaErrorKind
LuaError：限额构造错误，lua模块中pub结构体--未重导出，message不做512字节截断
    kind: LuaErrorKind--私有
    message: String--私有
    new(kind: LuaErrorKind, message: impl Into<String>) -> Self
        构造错误：私有关联函数，保存字段
    impl Clone + Debug + PartialEq + Eq for LuaError
    impl fmt::Display for LuaError
        fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result
            格式化：Debug分类、冒号、消息
    impl std::error::Error for LuaError
LuaTaskError：异步监督错误，crate公开结构体
    source: AsyncTaskError--私有字段
    impl From<AsyncTaskError> for LuaTaskError
        from(source: AsyncTaskError) -> Self
            转换监督错误：保存source，不带tool请求ID
```

## 逻辑

```text
路由校验失败：
    直接MclEffectReply Err加AgentFailure，不受ends_the_turn的两个分类限制
准备或同步执行失败：
    finish_tool_call把任意错误转为failed=true的Tool消息，缺pending时不回复
Lua guard收到执行结果：
    仅Aborted和RunnerFailed走效果错误加AgentFailure分支，其余错误是failed=true的Tool消息
    成功输出中的JSON信封仅由guard解包，run_lua_tool和bin本身返回原始String
Lua错误：
    RuntimeError直接归Aborted不代表一定是工具主动要求停止；其他mlua外层变体归ExecutionFailed
两层timeout：
    execute_prepared_lua_tool外层为ExecutionFailed，spawn_tool_runner内部为RunnerFailed
    不据此保证每次超时都会终止轮次
runner非零退出：
    stderr中的RunnerFailed被runner_failure降级为ExecutionFailed，与直接spawn失败不同
取消：
    CancelToolTurn只删除pending记账，不取消已经开始的执行，不抑制Lua guard后续回执
```

## 源码覆盖与测试边界

本文核对13个src文件：五个根模块、七个handler文件、一个bin。上述清单涵盖全部非测试生产类型和函数，
测试函数不作为运行时API逐个重复定义。已读取内嵌library_tests、environment_tests、pty::tests与lua::runner_tests。
它们包含全局根解析、环境过滤、PTY结果/截断/超时、信封与错误映射、runner字段、shell注入测试。
这些测试的存在不证明大输出不阻塞、任意后代必被回收、取消会杀任务或直接run_lua_tool自动受沙箱隔离。
本次是文档核对与重写，没有运行测试，也没有修复上述代码限制。
