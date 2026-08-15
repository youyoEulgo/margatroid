# ShellPlugin

## 类型

公开：
```text
ShellPlugin：Shell资源注册与异步脚本执行Plugin，公开结构体
    home_root: Arc<PathBuf>--主目录Shell资源根，例如~/.margatroid/shells
    limits: ShellExecutionLimits--定义读取、参数、输出和执行时间限制
    open(home_root: impl Into<PathBuf>) -> Result<Self, ShellError>
    with_limits(self, limits: ShellExecutionLimits) -> Result<Self, ShellError>
    impl Plugin for ShellPlugin
        构建：要求RuntimePlugin、AsyncRuntimePlugin和ToolPlugin已安装；挂载注册、准备和异步执行System

ShellExecutionLimits：Shell单次执行限制，公开结构体
    max_definition_bytes: usize
    max_script_bytes: usize
    max_argument_bytes: usize
    max_output_bytes: usize
    max_execution_time: Duration
    new(...) -> Result<Self, ShellError>
    impl Default for ShellExecutionLimits

ShellRegisterRequest：Shell资源注册请求，公开事件
    id: String
    agent: Entity
    resource_id: ResourceId--必须使用type=shell
    impl Event for ShellRegisterRequest

ShellRegisterResponse：Shell资源注册结果，公开事件
    id: String
    agent: Entity
    resource_id: ResourceId
    result: Result<(), ToolError>
    impl Event for ShellRegisterResponse

ShellError：ShellPlugin构造错误，公开结构体
ShellErrorKind：InvalidRoot、InvalidLimits、AlreadyInstalled
```

## 资源格式

```text
shells/<scope>/<name>/<tag>/
├── shell.toml
├── input.schema.json
└── main.sh
```

`shell.toml`严格包含：
```toml
schema_version = 1
name = "sh"
description = "Execute a shell command."
```

`input.schema.json`必须描述一个包含`command`字符串字段的JSON对象。资源的ToolSpec来自这个
元信息和Schema；LLM只看到`shell:<scope>/<name>:<tag>`资源衍生出的Agent专属tool_name。

## 函数

```text
shell_register_system(world: &mut World)
    注册Shell资源：读取ShellRegisterRequest
    行为：验证type=shell；按项目、镜像、主目录查找完整资源包；有界读取元信息、Schema和脚本
        校验name、description、Schema和脚本正文；不执行脚本
        构造ToolTemplate并调用register_agent_tool
        tool_id固定为tool:builtin/shell:latest，resource_id保持shell资源ID
        成功或失败都发送ShellRegisterResponse

shell_tool_call_prepare_system(world: &mut World)
    准备Shell调用：只处理tool_id=tool:builtin/shell:latest
    行为：验证请求、读取AgentToolEnvironment和AgentIdentity、重新查找资源包
        解析arguments并按资源Schema验证，取得arguments.command
        创建ShellToolResponseGuard和PreparedShellToolCall后发送异步事件
        准备失败立即发送唯一ToolCallResponse

execute_prepared_shell(call: PreparedShellToolCall) -> Result<(), ShellTaskError>
    执行Shell脚本：私有异步System
    行为：使用`sh <package>/main.sh <command>`启动子进程，工作目录为Agent project_root
        stdout和stderr并行读取，分别有界保存并继续消费；采集退出码和输出
        非零退出码仍返回Ok(JSON结果)，因为它是命令结果而不是执行框架错误
        无法启动、超时或I/O失败返回ToolError
        ShellToolResponseGuard保证恰好一次ToolCallResponse
```

## 逻辑

```text
Workspace/BuiltinToolPlugin -> ShellRegisterRequest
ShellPlugin -> register_agent_tool(resource_id=shell:..., tool_id=tool:builtin/shell:...)

ToolPlugin -> ToolCallRequest { resource_id=shell:..., tool_id=tool:builtin/shell:... }
ShellPlugin -> PreparedShellToolCall -> AsyncRuntimePlugin
            -> sh main.sh <command>
            -> ToolCallResponse { result: JSON { exit_code, stdout, stderr } }
ToolPlugin -> AgentMessage::Tool
```

## 边界

```text
tool:builtin/shell:latest是内建执行器，不注册为LLM可见ToolSpec，也不进入Agent可见性。
只有shell:*资源会进入AgentToolMap和Inference ToolSpec。
ShellPlugin不检查命令权限；Shell资源是开发者主动安装的可信代码。
资源脚本接收一个完整command字符串作为第一个参数；脚本自身决定如何解释或限制它。
相对cwd解析到Agent project_root；绝对资源路径按项目、镜像、主目录顺序查找。
子进程不持有World；所有结果通过ToolCallResponse回到事件系统。
```
