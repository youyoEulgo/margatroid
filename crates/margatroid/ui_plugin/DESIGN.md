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

UiPlugin 在编译期把 `ui/dist` 嵌入二进制，并在 daemon 已有的 HTTP 服务上提供界面资源。
界面与 WebSocket 后端同源，因此前端默认连到它自己的来源，不需要外部地址配置。

# lib

## 类型

crate公开：
```text
UiAssets：界面资源集合，crate公开单元结构体--由 rust-embed 在建期扫描 ui/dist 生成
    get(path: &str) -> Option<EmbeddedFile>
        读取资源：rust-embed 生成的方法，路径相对于 ui/dist；未命中返回None

UiPlugin：界面插件，公开结构体
    schedule: String--插件所属Schedule；界面没有自己的System，该字段只为装配一致性保留
    new() -> Self
        构造插件：公开关联函数，Schedule取RuntimePlugin::UPDATE
    with_schedule(mut self, schedule: impl Into<String>) -> Self
        设置Schedule：公开方法
    router() -> Router
        构造路由：公开关联函数，返回 axum Router
        行为：注册"/"到index_handler、"/{*path}"到asset_handler；调用方用AppServerExt::add_http_routes挂载
    impl Default for UiPlugin
        默认构造：公开trait实现，等价于new
    impl Plugin for UiPlugin
        Plugin：公开trait实现
        build(self, app: &mut App)
            安装插件：公开方法
            行为：重复安装直接panic，否则写入UiPluginInstalled标记

UiPluginInstalled：插件安装标记，公开单元结构体
    impl Resource for UiPluginInstalled
```

私有：
```text
index_handler() -> Result<Response<Body>, StatusCode>
    返回首页：私有异步函数，读取index.html

asset_handler(Path(path): Path<String>) -> Result<Response<Body>, StatusCode>
    返回资源：私有异步函数，按路径读取ui/dist下的文件

asset_response(path: &str) -> Result<Response<Body>, StatusCode>
    构造资源响应：私有函数
    行为：从UiAssets取值，未命中返回404；命中则带上按扩展名推断的Content-Type返回正文

content_type(path: &str) -> &'static str
    推断Content-Type：私有函数，按扩展名映射；html、js、css、json、svg、ico、
    png、jpg、jpeg、webp、woff2、woff、ttf分别对应自己的类型，其余回落到application/octet-stream
```

## 逻辑

```text
装配：
    daemon 先 add_plugin(UiPlugin::default())，再用 AppServerExt::add_http_routes(UiPlugin::router())
    把路由并入 RouteRegistry；UiPlugin 自身不注册 System，也不直接绑定端口

请求：
    浏览器 GET /            -> index_handler  -> asset_response("index.html")
    浏览器 GET /assets/x.js -> asset_handler  -> asset_response("assets/x.js")
    浏览器 GET /ws          -> 不经过本插件，由DtoPlugin注册的WebSocket路由处理

前端连接：
    ui 的默认后端地址取同源（页面协议与主机拼出 /ws），因此从 daemon 地址打开即可直接工作

历史展示：
    新message记录从content取得正文，从payload取得type、reasoning、tool_calls、usage、stop_reason与工具关联数据
    Error从content还原message文本，Inject保留payload中的完整嵌套messages并递归展示
    兼容旧payload中的完整消息正文，不修改旧数据库行；普通消息只在对话区渲染，MCL审计在右侧可展开面板渲染
    时间使用created_at_ms；0或无效时间显示Time unavailable，不渲染为epoch本地时间

上下文用量：
    从AgentStateDto.context_window_tokens取得与Lua环境一致的启动模型窗口快照；缺失时不猜测
    从当前Agent历史中最近一条assistant取得usage.input_tokens加usage.output_tokens，嵌套Inject递归处理
    不将cache_hit_tokens重复相加，不以历史累计In/Out替代上下文占用；最近assistant无usage则显示未知
    百分比为上述最近用量除以窗口；圆圈绘制比例限制在0到1，文字保留真实百分比，未知显示横线
    该值是最近推理返回的usage，不声称实时计量之后新追加的消息或工具输出

资源面板：
    左下角收起态只显示小三角与资源面板文字，不排列可用工具名称
    点击切换展开；展开态保留Sandbox选择及统一Tools可见性列表，不改变资源加载和调用语义
```

## 回合中止交互（已实现）

```text
状态与输入：
    state.sync同时携带status与turn；发送/中止可用性依赖status，不以turn或pending_command替代
    idle：允许发送用户消息，禁止中止
    working：禁止发送用户消息，允许发起中止；首次请求后可禁用重复点击并显示收口中
    working且turn为空：表示中止已请求但Driver尚未finish，不恢复发送按钮
    creating/stopping：不允许发送；不把非运行生命周期当作idle
    后端同样校验状态，拒绝working期间的新用户输入，不能只依靠前端禁用

中止控制：
    中止请求使用独立控制路由，不排入正在等待推理或工具的MCL FIFO
    请求针对当前完整Agent实例及目标turn，迟到请求不得中止随后新回合
    请求回执表示中止已受理，不代表工具已清理，也不表示Agent已经idle
    只有finish_turn后的状态同步确认idle，前端才恢复发送

assistant展示与本地中止：
    所有assistant统一按已有reasoning和content展示；cancelled只保留为领域stop_reason元数据
    不使用红色、Cancelled标签或Turn aborted.兜底；空内容保持为空，reasoning-only不伪造正文
    兼容历史拆分契约：正文仍从content取得，结束原因等元数据仍从payload取得
    取消消息允许显式记历史，但Driver不得把它注入推理上下文；前端渲染不替代这项执行规则
    点击中止即为当前完整Agent实例/turn安装临时显示屏障，并冻结已知请求ID；无需等待控制回执或HTTP取消
    屏障阻止该实例的未知请求分片，直到权威状态确认新回合；已知旧请求ID的有界tombstone跨新回合保留
    关联拒绝仅在相同实例/turn仍有效时解除临时屏障及该请求新加的tombstone，并显示真实错误
    受理回执保留屏障；真实finished仍用于权威历史收口，历史可对齐服务端接受的部分输出
    不伪造finished、Idle或清理完成；没有turn字段的未知旧请求在权威新回合后无法再区分，依赖HTTP取消与真实terminal边界

工具取消展示：
    仍按tool_call结果展示，原tool_call_id/tool_name保留，failed为true，正文为英文取消说明
    部分已完成工具保留真实结果，其他工具真实清理后返回取消结果；不声称撤销已有副作用
    CleanupFailed继续明确显示错误和不可继续状态，不伪装成正常取消assistant或idle
```

停止按钮使用 agent.turn.abort 独立控制协议；请求关联保留至状态确认不再处于该 Working 轮次，受理回执不提前恢复发送。

## 用户图片输入与历史

- 编辑器拖入图片与左下角加号菜单的唯一项目「上传图片 / Upload image」共用导入路径；支持多次导入和删除。
- 图片独占不换行、可横向滚动的一行；文字在独立行。无文字也可发送图片；发送失败保留草稿，不插入本地乐观历史。
- 每条消息最多 8 张，每张最多 5 MiB，总解码字节最多 20 MiB。仅接受 PNG/JPEG/GIF/WebP，校验 MIME 与文件签名，拒绝 SVG 等格式；错误简洁显示。
- 异步导入和发送固定捕获 workspace/agent，切换目标即清空草稿、图片和错误；迟到结果不能写入新目标或清空新草稿。
- 现有 MCL 发送契约为 `{type:'user',content:string,images?:[{media_type:string,data:string}]}`；data 为无 data URL 前缀的原始 base64，不转成 provider 格式。文件名只留在编辑器 UI。
- 历史以服务端记录为准，从 User 或 ToolCall payload.images 渲染独立图片行；图片数据不展示于活动、元数据或 JSON 文本。工具读图结果保留tool角色和原调用关联，不伪造User历史。
- 后端再次执行相同格式、大小和数量限制，浏览器校验不替代服务端边界。

## 边界

```text
UiPlugin只做静态资源服务，不解析业务请求；界面所需的全部交互仍走WebSocket
ui/dist不入库，构建前必须先跑 scripts/build-ui.sh，否则编译期扫描不到目录会报错
嵌入发生在编译期：改了界面必须重新构建并重新编译daemon，运行期不会读磁盘
路由只覆盖"/"与"/{*path}"；/ws由DtoPlugin注册，两者互不冲突
```

中止执行顺序：AgentPlugin 的 abort_agent_turn_system 固定运行于 RuntimePlugin::PRE_UPDATE，先于 UPDATE 的 MCL 与推理/工具 admission；UI 仅发送控制请求并等待权威状态同步，不承担清理完成判定。
