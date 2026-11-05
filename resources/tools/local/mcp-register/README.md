# MCP 注册工具

这是已确认、待实现的工具资源设计。[DESIGN.md](./DESIGN.md)只记录板块、逻辑伪代码及持有关系。本目录尚未交付工具 manifest、main.lua 或 bin/mcp-client；文档不表示 MCP 工具已经可运行。tool_inject、process_open 是已确认但待实现的宿主接口；task_send、task_receive 及 await_any 是已有能力。本次仅整理文档，不修改接口逻辑，不增加宿主函数，也不变更 System/Event；任何 System/Event 变更均需另行许可。

Margatroid 不实现或识别 MCP 协议。协议处理限于工具 Lua 和同目录独立编译的 mcp-client，不新增 Margatroid 领域或专用服务插件。

## 预期资源布局

```text
mcp-register/latest/
├── tool.toml          -- 注册工具的正常名称、描述与资源清单
├── input.schema.json  -- 注册 MCP server 的参数 schema
├── main.lua           -- 同一脚本内的注册分支与使用分支
└── bin/
    └── mcp-client     -- 独立交付的编译产物
```

资源清单声明依赖和运行需求；宿主不安装依赖、不编译或下载二进制。运行受所选沙箱限制，网络或可执行文件不可用时返回明确工具错误，不绕过沙箱。

## 入口与调用

注册入口 mcp_register 和动态入口 mcp_a、mcp_b、mcp_c 指向同一资源，不复制 Lua、不创建专门 MCP 执行器。动态入口的 alias、description、input_schema 来自发现结果及 Lua 映射；各入口按自己的 schema 校验 arguments。宿主固定动态入口的 owner_task_id 为注册任务 A，并绑定资源 path、Agent/VM 代际；binding 是 Lua 生成的不透明数据。owner_task_id 和 binding 不属于模型参数，也不接受调用者改写。

每次调用先 configure 再 execute；configure 无 IO/进程能力，连接放在 execute 的注册分支。分支只根据 context.owner_task_id 是否存在判断，避免远端工具参数恰好与注册 schema 同名时误判。A 的 timeout_ms 为 0；B 示例为 30000，由工具作者声明，也可按参数计算。

A 完成连接、初始化和 tools/list 后，一次原子注入全部定义，随后 task_return 注册成功，保持未 finish，继续持有 Lua VM、进程和会话。B 运行同一脚本，通过 task_send 请求 A，通过自身 task_receive 等待匹配响应；获得实际结果后 task_return，立即 task_finish。B 不能提前 return“已提交”，否则会成为已 return 的后台任务，脱离本轮未 return 任务的中止清理；return 与 finish 之间不启动长工作。

## 字节协议与并发

process_open 提供原始 stdin/stdout/stderr 管道，不使用 terminal/PTY；宿主只传字节。Lua 使用 resource_dir 定位二进制，通过 argv 数组传参。

Lua 与 bin 之间选择 JSON Lines：stdin 每行一个带唯一 id、operation 的请求；stdout 每行一个含原 id 的响应，允许乱序；stderr 独立输出诊断。完整换行帧到齐前不解析，必须支持一块含多帧、一帧跨多块及 UTF-8 跨读取切分。单帧、累计缓冲、在途请求和写队列均须有工具预算；超限明确失败，不截断后伪造合法响应。外部 MCP 的传输、版本协商和错误处理全部在 bin 内，内部 JSON Lines 不必等同于外部 MCP wire。

A 维护 pending[id] = reply_to，先登记再发送，响应只消费一次；未知或重复 id 丢弃并给出协议诊断，不误投其他 B。task_send 成功仅表示入队，完成以匹配响应为准。B 的响应走各自输入通道，不使用 task_read 共享游标。

A 发送请求后继续接收其他 B 请求及子进程输出，不等待单个远端调用结束。mcp-client 支持多个在途 MCP 请求，按原 id 返回，结果可以乱序；独占管道读取用于保持字节和路由正确，不能把请求串行化。

写入背压不能堵住 stdout 读取。A 使用有界写队列和唯一写协程，队列满时明确拒绝对应请求，不在接收循环中无限等待写入。stdout 读取、stderr 消费或明确舍弃、进程退出观察与任务接收并发推进，避免子进程管道堵满。这些安排使用 Lua 异步协程及已有 await_any，不引入新的 System/Event 或宿主 MCP 请求类型。

动态入口沿用普通 MCL 工具批次机制：先预发全部 ToolExecuteRequest 及各自 oneshot，再并发收齐并按原顺序返回。每个 B 有独立 ToolTask、reply 与输入邮箱，不互相消费结果。

## 失败、取消与生命周期

连接或发现失败不注入；注入冲突整批失败，关闭本次子进程并结束任务。注入后、注册 return 前被中止，按未 return 任务规则终止并清理 A 所属入口，避免半注册。注册成功 return 只表示注册完成，会话后续断开仍须失败并清理。

B 调用失败时，按原 call_id/alias 返回 failed 并清理 B，A 和其他 B 继续。未 return 的 B 超时或被中止时，由宿主原子强制 return 并清理；A 迟到发送得到 TaskMissing，只丢该关联。工具可自行定义带 id 的 cancel 消息由 A/bin 处理；这是原有可选工具内部协议，不要求 Margatroid 解析 MCP 取消，也不保证本地中止撤销远端副作用。

A 的 bin 断开、EOF、退出或出错时，停止新受理，逐一回复 pending 失败并 finish，不能无限停在 task_receive。A 失联或 finish 时，任务监督直接删除 Tools 中 owner_task_id=A 的记录，并遍历同一 ToolTasks 中依赖 A 的等待 B，逐个返回明确工具错误并清理；无需另设依赖管理器，也不依赖 A 还能发送消息。

A 真正 finish 前须完成子进程及后代、管道、VM 与临时资源清理，随后从 ToolTasks 移除；清理失败保留监督，不保存已 finish 条目。Agent/Driver 重启不持久化动态定义或任务，不恢复旧标记；既有未 finish 任务仍受当前重启限制，关闭沿原流程真实清理。

## 工具集合与持有关系

IMPORT TOOL 和 tool_inject 都向 Agent 的同一 Tools 集合增加记录。每条记录有 alias、description、input_schema、path；动态记录另有 owner_task_id、binding。每次推理直接遍历 Tools 构造 schema；调用时按请求名称查当前 Tools，存在即按记录执行，不存在即报工具错误。无需额外工具列表、配置读取或执行脚本来构造 schema。

Agent 持有 Tools 与 ToolTasks。A 持有 Lua 并发循环、pending、写队列和通向 mcp-client/MCP server 的进程会话；各 B 持有各自请求响应和通用 owner 依赖。A 清理时移除它拥有的动态入口，并释放依赖它的等待 B。
