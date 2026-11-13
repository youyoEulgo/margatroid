# DTO

本轮同步架构是目标设计，尚未实现。已实现源码先提交为e24862b；共享API资料对应当前45种网络消息，不把下面的新投递路径当作已部署行为。字段、函数及职责见[DESIGN.md](DESIGN.md)。

## 统一入口与共享槽位

同步分为异步领域监控和同步领域探针。二者都持有同一Clients Arc槽位，每次更新直接按当前注册身份与投递配置筛选ClientSender，入队到该连接原有的发送队列。Axum唯一writer实际发送WS；没有额外sync通道、统一分发循环、WebSocketMessageSend或每帧调查。

同步领域的探针是唯一写入口的固有职责。入口接收目标、具体修改以及必需的更新上下文；所有派生句柄、World扩展、Lua宿主和异常/清理路径都必须经过该入口，底层可变字段保持私有。只有成功且可观察状态变化才推进版本并同步。读取、拒绝、重复值和脚本游标变化不制造同步。

异步领域长期等待真正变化。Workspace定义与Driver UI使用notify磁盘监控，过滤无关/Access事件并合并连续构建写入；数据库记录使用确认COMMIT的watch高水位，不能用SQLite/WAL文件事件冒充事务成功。读取稳定数据后直接使用共享槽位发送，不回World取sender、不唤醒帧。

## 九种sync来源

| 接口 | 更新入口或监控 | 发布形式 |
|---|---|---|
| workspace.sync | 启动时建立定义磁盘监控；Workspace生命周期、成员、故障及定义缓存统一更新入口 | 最新完整列表 |
| agent.sync | Agent生命周期/故障及Tools、沙箱、模型公共视图的统一更新入口 | 最新完整列表 |
| agent.turn.sync | Agent的StartTurn、FinishTurn、AbortTurn统一写入口 | 当前turn或null |
| agent.task.sync | Task的Register、Append、Replace、Return、AbortReturn、SetError、CompleteCleanup统一入口 | 最新完整活跃任务集合及保留窗口 |
| agent.ui.sync | 安装Agent实际Driver来源时启动UI磁盘监控 | 稳定完整集合、缺失或错误 |
| agent.log.sync | 实际AgentStorage成功提交监控 | 有序新增日志 |
| agent.mcl.sync | 实际AgentStorage成功提交监控 | 有序新增审计 |
| agent.message.chunk.sync | 推理接受门确认片段后的唯一流出口 | 有序片段 |
| agent.message.full.sync | 推理终态出口；消息持久提交监控 | 完整生成结果或真实持久消息 |

状态类只排入轻量StateKey，writer通过独立Arc读取能力取得最新快照；同对象尚未发送的更新可以合并。这样统一写入口不在每个小输出片段上复制和编码整个任务历史。具体读取及大编码可在有界在途blocking worker执行，每连接最多准备一个快照，期间仍处理入站请求及取消。这个pending表属于每连接原有队列，不是sync中转通道。

chunk和记录不采用状态覆盖。新连接获得当前状态源的初始化，不重放历史或旧chunk；开始于生成中途的连接可能从后续sequence加入，最终full及显式历史查询负责完整结果，不承诺缺失前缀重放。

## 归属、顺序与失败

注册成功把ClientIdentity原子发布到唯一槽位，World组件引用同一身份。断连先移除槽位，再做World清理；异步线程使用当前身份，不需要World借用。选出sender后连接恰好断开属于正常竞争，Closed不能恢复已结束连接。

各领域在自己的更新门内提交状态及单调revision，释放领域锁再通知ClientSender。来源epoch区分Agent重启或资源替换，完整Entity及epoch只作内部归属。准备快照时读取实际版本，旧来源或旧版本不覆盖新状态。读回调只捕获Weak共享数据，避免上下文和发布索引形成Arc环。

按既定配置筛选：状态与持久记录使用backend_state，推理出口保持webui类别语义；匹配按ConnectionId去重。慢连接队列满或关闭时取消连接，有序增量不静默丢帧。编码/32MiB预算失败明确诊断并关闭受影响同步连接；普通查询沿原res错误返回，不能无条件重试制造忙循环。

## 生命周期与接口边界

Workspace定义监控覆盖新增、修改、删除和原子替换。自动读取与显式reload、启动前reload使用同一全量校验和发布入口；改文件更新候选定义及workspace.sync，不自动启动、停止、重启或改写运行实例自有定义。列表同路径优先展示有效候选定义，来源删除时用运行实例自有定义补位，展示值不冒充运行实例已换用新执行材料。非法文件保留最后有效集合并报告现有可表达错误；源文件移除的运行实例继续展示并允许停止。

UI监控在实际Driver来源安装时建立，记录监控在真实AgentStorage初始化时建立，均跟随本次来源取消与真实join。查询只读取既有状态，不创建监控或订阅。重启先使旧来源失效，取消并等待旧worker结束，之后安装新来源；关停等待所有监控与在途快照准备真实结束。

DTO移除dto_sync_system以及WebSocketMessageSend、ConnState、SyncState的旧投递/轮询结构；普通dto_route_system与abort_control_system保持。业务注册、生命周期、MCL及原oneshot链不改成同步消息；AgentException仍由Agent领域处理，并通过其统一更新入口发布故障。

本轮不改45种wire字段，不加入VM身份，不引入HTTP Driver资源接口。具体领域类型只依赖自身core及client_core能力；DTO提供具体编码函数，compose注入typed绑定，业务plugin之间不互相依赖。

## 设计验收

- 没有活动帧时，磁盘变化、Task输出、Return/错误/移除及turn变化仍可到达WS。
- 无实际更新时没有ECS同步事件、帧唤醒、SQL调查或自循环。
- 全部写能力通过唯一入口，无缺少同步上下文的生产构造、可选回调或直接可变字段旁路。
- 后连接接收当前状态与后续更新，身份筛选实时；旧来源、迟到准备和注册初始化都不能回滚状态。
- 大输出只有每连接每key一个待发送状态，不堆积多份完整窗口；具体增量按序、不覆盖。
- 查询不消费脚本输出游标、不隐式订阅，UI修改不重启Lua VM，定义修改不热改运行实例。
- 取消等待真实文件/SQL/快照worker退出；连接与来源退出后无Arc环、悬挂sender或重复清理。
