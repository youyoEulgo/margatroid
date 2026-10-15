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
    新message记录从content取得正文，从payload取得type、reasoning、tool_calls、usage与工具关联数据
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

## 边界

```text
UiPlugin只做静态资源服务，不解析业务请求；界面所需的全部交互仍走WebSocket
ui/dist不入库，构建前必须先跑 scripts/build-ui.sh，否则编译期扫描不到目录会报错
嵌入发生在编译期：改了界面必须重新构建并重新编译daemon，运行期不会读磁盘
路由只覆盖"/"与"/{*path}"；/ws由DtoPlugin注册，两者互不冲突
```
