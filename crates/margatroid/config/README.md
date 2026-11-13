# Config

config_core定义MargatroidConfig和配置值，只依赖mecs及通用库。config_plugin读取并校验config.toml，安装Resource，不依赖其他领域core或plugin，不创建System、查询事件或文件监听。

```toml
[server]
bind = "127.0.0.1:3939"
allow = "localhost"

[log]
level = "info"

[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]

[tools]
lua_vm_memory_limit = "2G"
task_output_buffer_window = "128M"
```

ConfigPlugin::open在启动阶段读取，build只发布已校验值。filter若提供须非空，完整替代level；tracing表达式由应用装配日志设施时校验。日志目标决定已产生记录的投递，不改变过滤条件。

allow缺省localhost，支持all或IP/CIDR数组；回环bind不能搭配放行非回环的策略。带Origin的请求要求Host同源。目标支持broadcast、type和name，两组均非空且各自不重复。入口策略不等于客户端身份认证。

模型路由归Inference，Workspace路径及统一定义/启动缓存归Workspace，Agent setting.toml归Storage。Config不复制这些定义或充当万能配置表。各plugin在边界转换OutboundTarget，不让config_core引用DTO契约。整体重载不在此定义独立流程。

`[tools]` 接受两个容量字符串：`lua_vm_memory_limit` 默认 2 GiB，`task_output_buffer_window` 默认 128 MiB。整个段或任意字段均可省略，compose 只从工具领域的 `ToolExecutionLimits::default()` 补齐默认值。单位后缀大小写不敏感，按 1024 进制解释：`K/M/G/T`，可选 `B` 或 `iB`（例如 `2G`、`128MiB`）；也可使用不带后缀的正数字节数。序列化使用可无损表达的最大单位。只接受正整数容量字符串，零、负数、小数、未知单位、整数类型及溢出均拒绝。旧的 `[tools.limits]`、`memory`、`cache` 和字节命名字段严格拒绝。

`lua_vm_memory_limit` 只限制每个工具Lua VM的分配器，覆盖顶层加载、configure和execute。它不限制宿主Driver VM，也不限制Rust缓冲、helper、Bash、MCP服务或其他外部子孙进程。已删除进程树总额度及total_task_memory_limit字段，不再创建内存cgroup或systemd scope，也不保留旧字段兼容。

`task_output_buffer_window` 是每个任务在宿主中保留的输出窗口，超限自动滚出旧内容，不是最终返回结果的容量上限，与Lua内存限额独立。工具 timeout 继续由工具 configure 配置。

配置在启动时读取；修改数据目录中的 `config.toml` 后重启 daemon 即生效，无需重新编译。config_core 只持有自有 `FileToolConfig`，不依赖工具领域也不复制工具默认值；compose 转换并校验完整预算后才安装 ToolPlugin。配置失败保留配置文件路径，程序化零值错误标明对应的 `tools` 字段路径。

单VM内存配置名称为lua_vm_memory_limit，旧task_memory_limit和total_task_memory_limit均明确报未知字段；不提供别名或静默忽略。工具内部对应字段为lua_vm_memory_bytes。
