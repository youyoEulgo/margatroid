# UiPlugin

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
