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
    router() -> Router
        行为：提供index与嵌入资源路由，挂到已有Server，不另开端口。
    impl Plugin for UiPlugin
        build(self, app: &mut App)
            行为：拒绝重复安装，写入安装标记；不注册业务System。

UiAssets：编译期嵌入资源
    get(path: &str) -> Option<EmbeddedFile>
```

# handler

## 函数

私有，Rust：
```text
index_handler() -> Future<Output = Response>
    行为：返回嵌入index.html。
asset_handler(path: String) -> Future<Output = Response>
    行为：查询嵌入资源，设置Content-Type，未命中返回404。
```

前端，TypeScript伪代码：
```text
handleServerMessage(message: ServerMessage): void
    行为：校验网络形状并更新权威配置、实例及历史；delta先检查实例、request、sequence和冻结标记。

startWorkspace(path: string): Promise<void>
    行为：发送workspace.start {path}，等待匹配回执，不上传展示缓存。

stopWorkspace(address: string): Promise<void>
    行为：发送workspace.stop，等待权威状态，不乐观删除实例。

abortTurn(): Promise<void>
    行为：立即冻结当前request/instance的流，发送agent.turn.abort；保持Working直到权威sync确认Idle。

sendUserMessage(): Promise<void>
    行为：固定当前目标；按INJECT及EFFECT wake契约发送，回执失败保留草稿，切换目标隔离迟到结果。
```

# types

## 类型

前端：
```text
WorkspaceConfigDto：展示条目--path、cache
WorkbenchState：权威快照--workspace_definitions、workspaces、agents、histories
StreamState：显示累积--request、instance、sequence、content、reasoning、frozen
ComposerState：草稿--目标、文本、图片及等待回执状态
```

# 持有关系

```text
已有HTTP Server
└── UiPlugin静态路由

浏览器Workbench
├── WebSocket连接
├── Workspace路径及展示缓存
├── 运行实例与权威历史
├── StreamState（临时显示）
└── ComposerState
```
