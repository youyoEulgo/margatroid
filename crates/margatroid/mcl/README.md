# MCL

mcl_core定义语法树、队列、请求和结果，只依赖mecs及通用库。消息在MCL契约中使用结构化值，执行时由mcl_plugin转换并校验所属领域消息类型；core不引用其他领域core。mcl_plugin依赖address和所需领域core，不依赖其他plugin。

本文同步本轮实现契约，不表示运行中的daemon已更新。IMPORT TOOL只向Agent的唯一Tools集合增加alias、description、input_schema和path记录，不维护选择名单。inference与tool_call使用当前Tools，各只接收一个消息数组输入；tool_call要求数组恰含一条Assistant。tool_inject、原始进程通信和MCP协议由通用工具宿主及工具侧处理，MCL使用现有指令执行领域调用。

## 指令

```text
IMPORT MESSAGE prompt:local/coder-soul:latest AS soul
IMPORT TOOL tool:local/bash:latest AS bash
IMPORT SANDBOX sandbox:local/workspace-write:latest AS workspace_write
CREATE TEMP BLOCK msg (instructions, compact_prompt, compact_context)
CREATE TEMP BLOCK entry (pending)
CREATE BLOCK session (conversation)
GET session.conversation
GET entry.pending[0]
INJECT soul TO msg.instructions
INJECT ? TO session.conversation[-1]
INJECT [] TO entry.pending[0,1]
CONFIG GET
CONFIG SET ? ?
CONFIG DELETE ?
EFFECT inference (msg.instructions, session.conversation)
EFFECT tool_call (session.conversation[-1])
EFFECT sandbox_use ?
EFFECT history_append ?
EFFECT start_turn
EFFECT finish_turn
EFFECT ready
EFFECT wake
```

Lua调用为mcl(target_entity, text, arguments)，arguments按顺序对应问号。来源及本次VM实例由lua_runtime_plugin中的mcl宿主绑定，不接受调用者伪造source。宿主只依赖mcl_core发送MclCommand并等待reply；唯一执行入口MclCommand要求调用方显式提供source_address，网络入口及Lua宿主传入自身真实地址，与source实体身份核对，MCL审计source保存该地址而非Entity编号；history_append消息按payload.type保存type，不保存调用来源；mcl_plugin不定义Lua函数。消息参数要求完整消息数组；marker形式分别为EFFECT inference ?和EFFECT tool_call ?，只占一个参数，不接受第二个工具alias数组。配置路径和值分别占一个参数。

## 消息Block

Block字段只保存消息数组，创建时不写类型。普通Block存于Agent SQLite，TEMP按VM实例隔离。CREATE不存在则创建，存在且字段一致则打开，不清空内容；不返回created标志。

GET返回.Array.Message，单索引也返回数组。索引从0开始，-1为末项；非负范围左闭右开，负范围左开右闭，端点同号。多来源按书写顺序拼接，Prompt alias每次按路径读取。工具和沙箱alias不作为消息GET来源。

INJECT无目标索引时整体替换；单索引插入，非负插到元素前、负索引插到元素后；范围执行替换。空数组清空字段或删除选定范围。所有数据库来源和目标在同一事务中读取、校验并修改，失败保留原值。已提交操作不因稍后取消而伪称回滚。

## 设置

CONFIG GET返回.Config，读取当前Agent完整settings.toml。CONFIG SET ? ?传入点分路径及TOML值，没有键则新增，存在则整体替换，不追加或合并。CONFIG DELETE ?传入点分路径，删除指定键，不自动删除空父table。

SET创建缺失的中间table；中间节点是非table时返回错误。DELETE目标不存在视为无修改成功。键路径不支持数组索引，不用nil或空数组表示删除；TOML不支持的值报错。日期时间等TOML值由宿主桥接明确转换，不把普通字符串猜成日期。

配置只访问当前Agent设置文件，不接收任意文件路径。修改串行执行并以完整文件替换持久化；失败保留原文件，不在Agent上缓存第二份设置正文。模板只在Agent设置不存在时初始化，重启不覆盖已有设置。

## 导入与效果

IMPORT校验种类、依赖和alias后，在对应资源领域登记；TOOL保存路径、描述及schema的完整记录，只增加工具，不另设可见名单。alias在同Agent内跨类别唯一；冲突不覆盖旧登记。工具描述及schema在导入时缓存于Tools，文件metadata变化不会隐式更新记录；执行按记录path读取程序，按记录schema校验参数。MCL不保存通用ImportBindings、ResourceID或工具/沙箱正文仓库。

| EFFECT | 参数 | 返回值 |
| --- | --- | --- |
| inference | 消息数组（单输入） | .Inference.message、.Inference.stop_reason |
| tool_call | 单条Assistant组成的消息数组（单输入） | .Array.Message |
| sandbox_use | 零或一个沙箱alias组成的数组 | Unit |
| history_append | 消息数组 | Unit |
| start_turn | 无 | Unit |
| finish_turn | 无 | Unit |
| ready | 无 | Unit |
| wake | 无 | Unit |

inference和tool_call将请求交给所属领域并等待oneshot。tool_call先发出整批带独立oneshot的请求，再通过join_all收齐并保持输入顺序；保留原调用ID，每个工具调用错误均转换为failed工具消息，包括配置、参数、执行、超时、清理、任务panic、回复丢失和关联错误；不因单个失败丢失整批结果。无法解析出有效工具调用的命令错误与其他非工具错误，通过唯一回复出口转换为Error值，形状为{"Error":{"type":"error","message":"错误种类: 说明","request_id":"本次命令id"}}，不抛Lua宿主异常终止Driver。原始执行失败仍在MCL审计中记录succeeded=false。清理尚未证明时及时返回错误并保留后台监督，不将任务伪报completed或安全退出。sandbox_use选择alias，不保存长期策略正文；实际执行读取选定路径的策略。history_append显式保存消息；其他操作不自动追加消息历史。指令审计与消息历史分别记录，不把审计参数投影成重复消息。两类历史以Agent内sequence标识并排序，不设历史key；content由调用方提供并与payload独立原样存取。

start_turn和finish_turn调用agent_core同步扩展；ready和wake调用lua_runtime_core扩展。ready只允许当前Driver对本人调用。wait是Lua宿主函数，不是MCL指令。上述调用由plugin组合，不让core之间互相依赖。

## 队列与取消

每个目标一个FIFO，一次只推进一个队首，异步等待不阻塞其他目标。System只路由，Handler完成校验和推进；闭包不跨await借用World。完成后通过原oneshot回复，不额外建立MclCommandResult中转事件。

插件仅允许UPDATE或POST_UPDATE，拒绝无法证明中止先行的自定义schedule。回合中止固定PRE_UPDATE同步控制，不进入FIFO，也不关闭整个队列。先清turn，再取消当前推理并原子强制return、终止清理未return工具；已return后台任务保留。inference/tool_call在实际受理时复查turn；无turn或请求turn不匹配时，Inference返回空assistant的Cancelled响应，Tool返回保留call_id/alias的failed中止结果且不创建任务，真实失败规范为工具失败结果或error消息。取消Assistant不进入推理上下文，Driver可显式保留历史；finish_turn只有本轮真实执行已收口才结束回合，尚未收口时立即返回error并保留句柄和状态，不循环等待；Driver在清理完成唤醒后重试，不能让旧完成覆盖新回合。

关闭队列拒绝新命令、取消排队和在途操作，仍等待文件worker、领域任务和回执真实收口。drain成功不只表示发出了取消；清理失败保持错误，不以丢弃Future代替退出证明。
