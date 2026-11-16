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

前端，TypeScript伪代码（resolve_workspace_resource为待接入目标，其余按现有界面职责）：
```text
resolve_workspace_resource(source: string, workspace_address: string, backend_endpoint: string, document_path: Option<string>) -> Result<string, ResourcePathError>
    行为：从当前消息所属Workspace取完整Address，不用异步回调执行时的新选择覆盖消息原归属；后端origin来自实际连接endpoint，ws/wss分别转换http/https。
        普通消息中的./foo及foo以Workspace工作目录为基准；渲染已知工作目录内Markdown文件时，以明确document_path的父目录为基准；未知文件来源不猜主机绝对路径。
        根URL为origin + "/" + encodeURIComponent(workspace_address) + "/"；相对路径按URL规则解析，保留查询和fragment，文件路径段各自编码，验证最终URL仍在同一Workspace根下。
        合法的子目录../可在前端归一化，但归一化结果不能越过Workspace根；后端直接收到的.及..段一律拒绝；不得仅改全页base标签影响平台JS/CSS或Driver自身资源。
        data:和明确http(s):保持原目标；//、file:、主机绝对路径等不作为Workspace相对路径推断；缺少Workspace归属时显示无法解析，不向daemon根请求裸文件名。
        Markdown图片及工作目录文件链接使用该URL直接HTTP读取，不通过read_image、工具任务或临时turn；Driver ui_path自己的资源仍走agent.ui，不混用两类根目录。

handleServerMessage(message: ServerMessage): void
    行为：校验网络形状并更新权威配置、实例及历史；delta按address、generation、sequence及冻结标记处理。

startWorkspace(path: string): Promise<void>
    行为：发送workspace.start {path}，等待匹配回执，不上传展示缓存。

stopWorkspace(address: string): Promise<void>
    行为：发送workspace.stop，等待权威状态，不乐观删除实例。

abortTurn(): Promise<void>
    行为：立即冻结当前address/generation的流，发送agent.turn.abort；后端先清turn，再取消推理、强制return并清理未return工具，已return后台任务保留。受理ack不表示清理完成；Agent始终running，以权威turn sync确认当前turn已撤销及后续回合关联，不以长任务自然结束作为中止响应条件。

sendUserMessage(): Promise<void>
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
├── Workspace路径及展示缓存
├── 运行实例与权威历史
├── StreamState（临时显示）
└── ComposerState
```
