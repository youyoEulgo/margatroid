# Protocol

Protocol定义JSON信封、请求、回执、状态、历史和流分片。只依赖标准库及serde等通用库，不依赖领域core、plugin、address或World。领域转换、寻址和请求关联由DtoPlugin完成。

## 网络消息

入站信封为`{type,id,message}`，id非空且最多512个UTF-8字节，字段使用snake_case。实体地址与文件资源定位符作为字符串传输，不使用内部Entity、组件或句柄。

请求包含连接注册、Workspace列表与启停重启、状态查询、历史、MCL和回合中止；响应携带原网络请求ID。Workspace列表由DTO同步投影启动时已加载的内存配置，不触发磁盘扫描或领域查询事件。配置修改通过Margatroid统一重载生效，不提供Workspace独立配置变更和刷新接口。

## 状态与消息值

state.sync包含workspace_definitions、workspaces、agents和histories。agent_instance是拒绝迟到消息的不透明输出标识，不用于入站寻址。

网络消息独立定义System、Inject、User、Assistant、ToolCall结果和Error。MCL命令执行失败通过result.Ok.Error携带type=error、message及request_id；外层Ok表示错误消息已经交付，不表示命令成功。工具调用失败通过Array.Message中的tool_call.failed=true返回，保留原调用ID。网络信封、注册及目标受理错误仍使用对应协议失败回执，前端须同时检查消息内错误，不能只检查外层Result。媒体放入独立data数组，当前元素为media_type与标准base64 data；支持PNG/JPEG/GIF/WebP，每条最多8张、每张解码后5MiB、合计20MiB。Rust DTO字段images通过serde重命名为data，线上不使用images或厂商replay字段。正文与reasoning各保留一份，厂商与API格式转换只在推理适配边界处理。

每种AgentMessage均可携带sequence，值为真实持久历史序号。尚未写入历史的消息不提供该字段，不生成替代ID；历史读取与已持久消息的Block读取保留同一序号。它不等于流分片的sequence，也不随数组位置变化。

历史记录按sequence、turn、type、source、kind、content、payload和created_at_ms排列；type仅消息携带，表示AgentMessage类型（如user、assistant、error），source仅MCL携带。kind区分message与mcl，不代替消息type。sequence在单个Agent内稳定标识与排序记录，前端结合Workspace和Agent上下文构造展示标识；不传输历史key。MCL的source保存真实调用方地址，不使用Entity编号；消息不输出source。content与payload独立保留，content按调用方原值传输，不从payload派生。时间0表示未知。流分片关联请求ID、Agent、实例及sequence；finished表示显示流终态，不表示领域操作或清理完成。

## 校验

协议只校验JSON、字段形状及预算。总消息上限32MiB；递归消息深度最多32、节点最多4096，非图片消息值最多1MiB。

ProtocolErrorDto包含kind和message；message最多512个UTF-8字节，不包含凭据、图片或请求正文。注册身份、目标归属、业务状态及在途去重不属于协议校验。

## 生命周期载荷

workspace.start与workspace.restart的message同形为{path}，id位于信封。workspace.stop使用运行实例的公开地址。后端按path重新读取定义，同时更新展示缓存；state.sync.workspace_definitions为{path,cache}数组，不用于启动。

Agent启动与重启的message同形为{workspace,name,parameters}，DTO转换为内部AgentStartParameters并通过oneshot等待结果；内部Entity、服务端派生的数据库及设置路径不由网络直接提供。Agent重启接口与前端是否显示按钮分别处理。

agent.turn.abort携带workspace、agent、agent_instance和turn_id。协议只是网络类型，仍由DTO经下一帧专属控制事件完成受理，不在Axum中直接取消。控制先同步清turn，再取消当前推理、原子强制return并终止未return工具；已return后台任务保留。TaskReadResult仅含replaced/output/error，finish真实清理后移除，后续read/send/finish返回TaskMissing，无完成缓存。状态使用starting/idle/working/restarting/stopping；沙箱选择和工具结果只带alias或调用关联，不恢复资源ID。
