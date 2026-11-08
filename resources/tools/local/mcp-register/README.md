# MCP 注册工具

本资源已提供 [latest/tool.toml](./latest/tool.toml)、[latest/input.schema.json](./latest/input.schema.json) 和 [latest/main.lua](./latest/main.lua)。Lua 通过统一对象的 `margatroid:task_*`、`margatroid:tool_inject`、`margatroid:process_open`、`margatroid:await_any` 和 `margatroid:json_encode/json_decode` 使用通用工具、任务与原始进程管道；MCP 连接、初始化、版本协商和传输由独立应用 `apps/mcp-client` 处理。

运行前须独立编译该应用，并将可执行文件放到 `latest/bin/mcp-client`。资源不会下载、编译或安装依赖；网络、进程及文件访问沿用宿主沙箱。`context.resource_dir` 必须指向 `latest`，Lua 用绝对资源路径和空 argv 调用 `margatroid:process_open({program=..., arguments={}})`，不使用 shell 或 PTY。

## 注册参数

注册入口接受以下两种形状；`server` 必填，`alias_prefix` 可选，其他字段拒绝：

```json
{"server":{"transport":"stdio","command":"/absolute/server","args":[],"env":{}},"alias_prefix":"mcp_"}
```

```json
{"server":{"transport":"http","url":"https://example.test/mcp","headers":{}},"alias_prefix":"mcp_"}
```

`args`、`env`、`headers` 可省略，发给二进制时分别补成 `[]`、`{}`、`{}`。环境变量和 HTTP headers 的值必须为字符串。HTTP URL 必须以 `http://` 或 `https://` 开头。`alias_prefix` 默认 `mcp_`，显式值须符合 `^[A-Za-z_][A-Za-z0-9_-]*$` 且不超过 64 字节。

动态 alias 为前缀拼接远端 name，将 name 中每个不属于 ASCII 字母、数字、下划线、连字符的字节替换成 `_`。原始远端名字保存在宿主绑定 `binding.remote_tool`，调用时原样发送。映射内碰撞整批失败；与现有 Tools 冲突由一次 `tool_inject` 原子拒绝，不保留部分注册，也没有额外 registry。发现结果的 inputSchema 原样用于动态定义；description 存在且不全为空白时原样保留，缺失、空字符串或全为空白时使用原始远端工具名作为 description，以满足宿主非空描述校验。

成功结果是包含 `task_id` 和 `aliases` 的 JSON 文本。注册任务 A 的 `configure` 超时为 0；A return 后仍持有进程和事件循环。动态入口 B 的超时为 30000 毫秒，根据工具记录传入的 `context.owner_task_id` 分支，注册时显式将入口owner绑定A，不根据模型参数判断。B 从 `binding.remote_tool` 取实际工具名，通过自身 `task_receive` 等待匹配 ID，取得结果后 return 并立即 self finish。

## 内部协议

每个 stdin/stdout JSON Line 必须包含字符串 `id`。stdout 只能输出协议帧；stderr 始终并发消费并明确舍弃。

```text
register: {id,operation:"register",server:{transport:"stdio",command,args:[],env:{}}}
       or {id,operation:"register",server:{transport:"http",url,headers:{}}}
success:  {id,result:{tools:[{name,description?,inputSchema:object}]}}
call:     {id,operation:"call",tool,arguments}
success:  {id,result:rawMCPCallResult}
failure:  {id,error:{code,message,data?}}
```

A 使用有界 FIFO 写队列、`pending[id]=reply_to` 和 `await_any` 同时推进 stdout、stderr、进程 wait、任务接收及写入。注册阶段也同时读取两个管道和观察退出，不等待单个远端调用完成后才接收下一次调用。完整换行帧到齐才解码，支持多帧同块、跨块和 UTF-8 字节切分。未知或重复响应 ID 产生任务诊断并丢弃。无效 JSON、无效帧形状、未完成帧 EOF 或预算超限会明确失败并结束 owner，绝不截断后继续解析。

当前 `await_any` 会取消未胜出的函数，因此依赖 `process:write` 在取消后保留当前帧和已提交偏移，再次写入同一个完整帧时续写。Lua 只有一个 writer，始终写稳定的队列首帧，完整写入成功后才移除该帧；stdin 背压期间仍并发读取 stdout、stderr、进程退出及新任务。

工具预算为单帧 128 MiB（含换行）、stdout 累计缓冲 128 MiB + 8192 字节、最多 4096 个在途调用、最多 4096 个排队帧且队列总计 256 MiB。不按工具数量截断或过滤目录；client 的完整发现定义预算为 64 MiB，宿主整批注入为 128 MiB。默认原始 stdout 累计预算为 256 MiB，Lua 内存为 2 GiB、指令预算为 6.4 亿，任务输入容量为 1024。宿主限制仍可能更严格；这些预算不覆盖或绕过宿主限制。超出调用队列或在途预算只拒绝对应 B；协议、进程或 IO 错误结束 A 并向所有 pending B 回复 disconnected。只有发送给已移除 B 的 `TaskMissing` 被忽略，其他发送错误触发明确诊断及 owner 清理。

## 结果与结束

MCP text 内容按顺序连接为工具文本；存在structuredContent时，能解析为JSON且与之结构相等的文本块不再重复输出，最终只附加一份structuredContent。对象字段顺序不影响相等性，数组顺序、类型、null与缺失字段、空对象与空数组均严格区分。普通说明、不同JSON和无法解析的文本原样保留；独立client bin仍完整透传原MCP响应。成功 MCP image 内容映射为现有 `{media_type,data}` 图像，不解码或改写 base64。其他内容通过 JSON 文本保留。`isError=true` 转成 `{ok:false,content:...}`，由既有宿主结果解析器设置 `failed=true`；由于宿主禁止失败结果包含图像，失败中的 image 块保留为 JSON 文本。图像格式、数量和字节限制继续由宿主结果解析器校验。

注册或注入失败不返回成功；A 结束时 self `task_finish` 位于 `pcall` 外，因为宿主 self finish 永不返回。A的子进程、管道和VM由通用任务清理回收；A清理会移除显式绑定它的工具记录，但不会级联取消已受理的B。B按自身超时、取消或工具内部返回结果结束。B 超时、取消或消失不会主动取消远端副作用；迟到响应只移除对应 pending。资源不新增宿主函数、System/Event、持久化记录或隐藏工具集合。
