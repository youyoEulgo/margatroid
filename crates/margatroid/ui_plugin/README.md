# UiPlugin

## 状态与设计范围

当前 catalog、注册启动、成员增删、工作区创建/改名/删除、成员改名、Driver 发现及后端目录浏览均已实现并完成恢复验证。验证仅覆盖已执行的 scoped、真实存储集成和隔离数据浏览器场景，不构成所有故障或环境的普遍保证。

## 工作区与会话侧栏（已实现）

```text
● workspace-a                         …
  ● coder                             …
  ● reviewer                          …
  + New session
● workspace-b                         …
  + New session
+ New workspace
```

- 顶层显示全部已登记 Workspace，点击工作区行切换展开/收起。Agent 直接作为该 Workspace 的子行，不再使用独立 Members 分区。多个工作区可以同时展开；重复状态同步不随意折叠，也不重置仍有效的选择。
- 每个 Workspace 和 Agent 左侧均显示一个小圆点：灰色表示未启动或过渡中，绿色表示正常运行，红色表示当前定义或运行出错。颜色来自权威目录/实例/成员失败，不靠数据库是否存在、历史是否有字或任意日志推断；另提供 title/aria 文本，不能只靠颜色表达状态。
- 每个实际 Workspace 和 Agent 行右侧提供 `…`。菜单只有 Rename 与 Delete 两项；点击菜单不触发行选中/折叠，支持键盘、Escape 和外部点击关闭。重命名/删除失败留在当前对话框并显示真实原因，不假删或假改名。
- 每个 Workspace 固定包含与 Agent 并列的 `+ New session` 行：外观、尺寸和缩进与 Agent 行一致，左侧为加号，右侧没有 `…`。点击弹出创建对话框，填写新 Agent 名称并从已有 image 中选择一项；此处 image 对应已安装 AgentDriver，发送规范 `driver:scope/name:tag`，不要求用户手输资源地址，也不新造 Driver 生命周期。
- Driver 选择来源为 daemon 发布的可用 Driver 目录（`state.sync.drivers`），包含尚未挂载到任何工作区的有效包；列表为空时明确提示没有可用 image，不能假造默认值。创建前名称冲突、资源有效性和修订由服务端校验。
- 顶层固定 `+ New workspace` 行，与 Workspace 并列、左侧加号、无 `…`。点击弹出名称和项目路径选择对话框；项目路径指 daemon 所在机器的已存在绝对目录。浏览器不能把上传文件名或仅有名称的目录句柄伪装为绝对路径；界面提供 Browse 后端目录浏览与绝对路径输入，不声称使用原生文件管理器。
- 新建工作区只登记空成员定义，不自动启动；New session 沿用持久成员规则：工作区未启动时只保存定义，工作区已运行时保存后尝试启动成员，启动失败仍保留已配置成员并显示红点/诊断。
- 删除有确认。Agent 删除移除定义并在必要时等待真实停止，默认保留数据库；Workspace 删除需要先停止，移入主目录内部回收区，不会不可逆销毁历史。重命名必须保留既有会话、Block 和历史，后端不能只改名字而丢失数据。
- 两级操作均以捕获的 Workspace/Agent ID 与 revision 为目标；异步期间切换侧栏不会把结果写入另一个工作区。成功后采用回执和后续权威目录更新，失败/冲突提示刷新，不乐观修改目录或运行状态。
- 取消左下角重复的 MCL 快捷入口，只保留右上角展开右侧面板的入口；这不删除 MCL 审计数据，也不要求取消右侧原有 MCL 查看能力。资源面板、图片输入和中止行为沿用原契约。
- 侧栏采用统一行高、缩进、字重、圆点和菜单位置，名称为主、路径/Driver 放在辅助提示中。保留窄屏抽屉、长名称省略和键盘焦点，避免把诊断、操作按钮和长技术说明堆到行内。

### 新建与编辑的具体交互

New workspace 对话框提供名称、当前项目路径及 Browse。Browse 打开后端目录浏览器：首次请求 `workspace.paths.query {}` 从 daemon HOME 开始，可输入绝对路径跳转、进入子目录、回到父目录、分页及“选择当前目录”。只选择文件夹，不上传本地文件；后端返回规范绝对路径，确认创建时再次校验存在性。路径不存在或无权访问时保留名称与路径草稿、显示错误，不创建项目目录。后端目录浏览已实现；它是 daemon 目录浏览器，不是浏览器本地文件上传或原生系统文件选择器。

New session 对话框提供名称和 image 选择列表，通过 `driver.catalog.query` 取最近有效快照，Refresh 走 `driver.catalog.refresh`；可用包未挂载也应出现。未选 image 不发送；清单错误可见，未验证/无效包不能当可选默认值。确认发送 workspace.member.add，名称冲突保持对话框并提示。

两级 Rename 只编辑名称，不编辑路径或 Driver。Workspace 运行时提示先停止；Agent 若在运行，由后端执行 stop/drain、关闭数据库并迁移。持久改名完成后，若所属 Workspace 仍运行则尝试启动新名成员；启动失败仍是持久改名成功，以红点和错误表达，不回滚成旧名。Delete 对话框明确区分移除成员并保留库，与要求用户先停止、再将整工作区移入回收区。所有对话框只在权威成功回执后关闭，pending 时禁用重复确认；失败保留用户输入。

工作区或 Agent 改名成功后，按回执中的新 ID 重新绑定选中项，移除旧 ID 的树、菜单及异步目标关联；不能把迟到旧结果写到新实例。被删条目退出当前选择但不臆造替代目标。展开状态只在当前连接本地保存，不把旧名当新对象的持久身份。重连和刷新使用权威目录纠正状态。

### 状态点与操作位置

灰色包含 stopped/creating/stopping 等未完成过渡；绿色为已确认健康运行（Agent idle/working，Workspace Running且无当前成员失败）；红色为配置错误、清理/启动/Driver失败或当前实例错误。空成员健康 Running 工作区可为绿色。旧轮次错误日志不让已恢复的新实例永久变红；后端确认恢复后清理对应过期错误。错误原因通过hover/focus或详情可访问，不能挤进每行长说明。

三点菜单严格只有 Rename/Delete；Start/Stop/Refresh放在已选工作区的独立紧凑工具区，不塞入菜单、不作为额外伪Agent行。左下MCL快捷入口删除，右上单一面板入口保留；右侧原有日志和MCL审计查看能力保留，不能因删除快捷入口而使审计不可见。

## 目录与权威状态（恢复后的基础契约）

界面注册后查询 daemon 的 catalog 并消费 `state.sync.catalog`；runtime workspaces/agents 与注册条目分开。停止和无效定义仍可见，已运行实例不能因磁盘配置无效而隐藏。零成员不选择 Agent，唯一成员可自动选中但请求仍携带显式 ID，多成员由用户选择。未运行不开放发送或中止，持久化成功不等于成员启动成功。

规范定义位于 `~/.margatroid/workspaces/<name>/workspace.yaml`，成员状态在 `agents/<agent>/state.sqlite`；客户端不扫描或写项目产物。Start 仅提交 `{workspace, expected_revision}`，不上传 YAML 或数据库路径。`workspace.member.result` 的 entry 可空、error 为可选结构化错误；删除工作区成功的 entry=null 且无 error，须以新目录确认条目消失，不能把成功删除误报成查询失败。

### 已执行验证与范围

UI 26 项测试（含 mounted 菜单/对话框及 store）和 type-check、lint、build 已通过。另在隔离数据上通过实际 Chrome DOM 菜单、后端文件夹选择、Driver 列表、创建/启动绿点、运行成员改名/删除、停止工作区改名/删除及右侧 MCL 检查。未执行真实模型推理；这些场景不等于所有浏览器、视觉状态或错误路径的普遍验收。

`UiPlugin` 把 `ui/dist` 在编译期嵌入 daemon 二进制，并在同一端口上提供界面。

```rust
use core_plugin::App;
use server_plugin::AppServerExt;
use ui_plugin::UiPlugin;

let mut app = App::new();
// ... 安装其它插件
app.add_plugin(UiPlugin::default());
app.add_http_routes(UiPlugin::router());
```

界面与 WebSocket 后端同源：

```text
http://127.0.0.1:3939/      界面
ws://127.0.0.1:3939/ws      WebSocket
```

`ui/dist` 不入库，构建前先执行 `scripts/build-ui.sh`。嵌入发生在编译期，因此改动界面后需要
重新构建界面并重新编译 daemon。

## 用户图片

- 将图片拖入编辑器，或点击左下角加号 →「上传图片 / Upload image」选择图片。两者共用校验与导入逻辑，可多次添加、逐张删除，也可仅发图片。
- 预览独占可横向滚动的一行，正文另起一行。支持 PNG/JPEG/GIF/WebP；每条最多 8 张、每张 5 MiB、合计 20 MiB（解码字节），拒绝伪装格式和 SVG。
- 发送沿用 MCL user 消息，图片为 `images: [{media_type, data}]`（无前缀 base64）；文件名仅用于界面。失败保留草稿，切换 agent/workspace 清空草稿并隔离迟到异步结果。
- 对话图片仅来自后端权威历史，User 和工具结果中的 images 均显示在独立图片行，不将 base64 展示为活动或元数据文字。工具读图结果保留工具身份与原 tool_call_id，不在 UI 伪造 User 历史。
- 发送期间编辑器显示当前等待的是注入回执还是唤醒回执；Activity 的 `ui.send` 记录阶段与图片数量，不记录正文、文件名或 base64。注入成功不等于模型已开始运行，必须继续收到 wake 回执。
- 回执被拒绝、连接关闭或现有 120 秒回执超时均会结束发送等待并保留草稿；超时不代表后台命令已取消，重试前应核对历史，避免重复注入。
- UI 回归测试以超过 1 MiB 的序列化图片负载走 WebSocket `onmessage` 与协议解析路径，验证回执前不唤醒、`request.failed` 拒绝后可重试及历史图片投影。该测试仍使用假 socket，不替代 daemon 的真实 WebSocket 大消息集成测试或浏览器解码/响应性测试。

## 回合中止交互（已实现）

- 发送与中止以同步状态为准：`idle` 允许发送；`working` 只允许中止；`creating/stopping` 不允许发送。
- 中止立即清空 `turn`，但收口期间仍为 `working`，不能提前开放新消息；只有 Driver 的 `finish_turn` 完成并同步 `idle` 后恢复发送。
- 中止通过独立控制请求发送，不进入 MCL FIFO；请求受理回执不是清理完成通知。
- 点击中止立即冻结当前实例/回合的流式显示，无需等待控制回执；已知请求 ID 的迟到分片跨回合继续忽略，未知请求在权威新回合前也不追加。HTTP 取消仍异步进行，不伪造 Idle 或清理完成。
- 历史 assistant 无论 `stop_reason` 是否为 `cancelled`，均按普通消息显示已有思考和正文；不加红色、取消标签或空内容兜底。权威历史仍可对齐服务端接受的部分输出。
- 关联中止请求被拒绝时显示真实错误，并仅对仍属于同一实例/回合的临时冻结解除；迟到拒绝不能改变新回合。
- 取消 assistant 允许记录历史但不进入模型上下文；取消工具结果仍保存历史与上下文，保留调用关联。
- 真正清理失败仍显示错误，不伪装成取消成功或可继续状态。

停止按钮发送 agent.turn.abort，并按请求 ID、workspace、agent 和 turn_id 核对回执；受理成功后保持禁用，等待状态同步确认 Idle。

中止执行顺序：AgentPlugin 的 abort_agent_turn_system 固定运行于 RuntimePlugin::PRE_UPDATE，先于 UPDATE 的 MCL 与推理/工具 admission；UI 仅发送控制请求并等待权威状态同步，不承担清理完成判定。
