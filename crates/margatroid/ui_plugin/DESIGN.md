Margatroid领域实现契约；网络由领域原生handler处理，mecs仅提供通用运行设施。

本文描述本轮源码实现的中止与ToolTask生命周期契约；不表示用户已部署或正在运行的daemon已更新。

# 伪代码格式

```text
板块使用一级标题，一行注明文件；类型和函数使用二级标题。
只写接口、字段和直接行为，不写独立逻辑段或过程记录。
持有关系在文末使用树形图。
```

# 板块格式

```text
# lib     src/lib.rs                    UiPlugin与嵌入资源
# handler src/handler.rs                静态资源响应
# types   ui/src/types/protocol.ts      网络DTO与前端状态类型
# handler ui/src/stores/workbench.ts    连接、请求、历史及流处理
# lib     ui/src/App.vue；ui/src/components/  界面与树形组件
```

# lib

## 类型

公开：
```text
UiPlugin：静态资源插件
    router() -> axum::Router
        行为：提供根路径、显式/assets/{*path}静态资源路由及原静态资源fallback，handler直接返回原生Response，完成自有State装配后返回Router<()>。
            /assets是平台保留前缀，优先于DTO的/{workspace}/{*path}匹配，不能让文件路由截获平台JS/CSS；/ws仍由DTO精确路由处理，静态fallback只查内嵌资源，不读取工作目录。
    impl Plugin for UiPlugin
        build(self, app: &mut App)
            行为：检查AxumHandle，拒绝重复安装，写安装标记；自行调用AppAxumExt::add_axum_routes(Self::router())，共用AxumPlugin监听端口；不注册业务System。
            行为：Compose只安装UiPlugin，无需手工注册router；不经过通用HTTP请求事件。
            行为：axum_plugin是Margatroid基础设施依赖例外，不允许依赖DTO等业务plugin；本轮不实现Driver WebUI。

UiAssets：编译期嵌入资源
    get(path: &str) -> Option<EmbeddedFile>
```

# handler

## 函数

私有，Rust：
```text
index_handler() -> Future<Output = Response>
    行为：返回嵌入index.html。
asset_handler(uri: OriginalUri) -> Future<Output = Response>
    行为：从原请求URI取得完整静态资源路径，保留assets/前缀，供明确静态路由与fallback共用；仅查询嵌入资源，设置Content-Type，未命中返回404，不自行打开Workspace文件。
```

前端，TypeScript实际接口（ui/src/lib/workspaceUrl.ts及stores/workbench.ts）：
```text
workspaceFileUrl(value:string,context:MarkdownResourceContext) -> string | undefined
    行为：context持Workspace完整地址、实际WebSocket endpoint及可选baseDir；缺少归属、路径为绝对/协议URL/含反斜杠则undefined；相对路径归一化且禁止越过Workspace根，按段encodeURIComponent，ws/wss映射http/https，构造/{编码Workspace地址}/{相对文件}并保留query/fragment。
    行为：Markdown图片/文件由实际renderMarkdown消费此URL，不生成工具调用或临时turn；独立http/data链接由渲染策略处理，Driver UI来源仍是agent.ui。

handleServerMessage(message: ServerMessage): void
    行为：校验网络形状并更新权威配置、实例及历史；delta按address、generation、sequence及冻结标记处理。

startWorkspace(path: string): Promise<void>
    行为：从所选path定位权威Workspace快照的address，发送workspace.start {address}，等待匹配回执，不上传展示缓存。

stopWorkspace(address: string): Promise<void>
    行为：发送workspace.stop，等待权威状态，不乐观删除实例。

abortTurn(): Promise<void>
    行为：立即冻结当前address/generation的流，发送agent.turn.abort；后端先清turn，再取消推理、强制return并清理未return工具，已return后台任务保留。受理ack不表示清理完成；Agent始终running，以权威turn sync确认当前turn已撤销及后续回合关联，不以长任务自然结束作为中止响应条件。

sendMessage(text: string, attachments: Attachment[]): Promise<void>
    行为：固定当前目标；按INJECT及EFFECT wake契约发送，回执失败保留草稿，切换目标隔离迟到结果。
```

# types

## 类型

前端：
```text
WorkspaceConfigDto：展示条目--path、cache
WorkbenchState：权威快照--workspace_definitions、workspaces、agents、histories
StreamState：显示累积--address、generation、sequence、content、reasoning、frozen
ComposerState：草稿--目标、文本、图片及等待回执状态
```

# 持有关系

```text
AxumPlugin（Margatroid基础设施）
└── UiPlugin::build注册的原生Router
    └── 嵌入资源handler -> Response

浏览器Workbench
├── WebSocket连接
├── Workspace完整Address、只读路径及展示缓存
├── 运行实例与权威历史
├── StreamState（临时显示）
└── ComposerState
```
