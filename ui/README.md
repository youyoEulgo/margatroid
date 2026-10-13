# Margatroid UI

Margatroid 的 Web 工作台，使用 Vue 3、Vite 和 Pinia，直接连接 daemon WebSocket。
提供聊天历史、显式 MCL 指令工作台、Agent 状态和运行日志；不包含旧动态可见性、Workflow 热插拔或 turn abort 路由。

## 启动

使用已有 Node.js 依赖（Node 22.18+ 或 24.12+）：

```sh
npm install
npm run dev
```

也可以使用项目指定的 Bun 执行安装与脚本。Vite 开发页面默认地址为
`http://127.0.0.1:5173/`。UI 默认连接当前页面同源的 `/ws`；如果独立运行 Vite，
在 **Connection & driver** 设置中指定实际 daemon 地址，例如 `ws://127.0.0.1:3939/ws`。

```sh
cargo run -p margatroid_daemon -- --data-root ~/.margatroid
cargo run -p margatroid_cli -- workspace up ../demo_workspace/margatroid-workspace.yaml
```

构建生产页面使用 `npm run build`。开发服务器和生产构建都不会自动启动 daemon。

## MCL 工作台与用户消息

默认输入模式是 **MCL command**，一次发送一条明确指令。Arguments 输入框必须是 JSON 数组，
其中每项也必须是数组，依次对应指令中的 `?` 占位符。无占位符时使用 `[]`。

例如，由 driver 自己定义 `driver.messages` 上下文路径时，可显式输入：

```text
INJECT ? TO driver.messages
```

Arguments：

```json
[[{"type":"user","content":"请检查这个项目"}]]
```

`driver.messages` 只是示例 driver 约定，不是协议默认字段。UI 不推断上下文路径，
不自动发送 wake，也不因为注入成功而假定已触发推理。

**Inject user message** 模式仅是上述单条指令的便利入口：先在设置中填写
`Driver context path`，UI 才会发送 `INJECT ? TO <配置路径>`，使用同样的二维数组参数。
该配置默认留空，只保存在当前页面内存。启动轮次、调用推理或修改 sandbox 均应使用
实际 driver 支持的显式 MCL 指令，而不是旧 `agent.message` 或 `agent.turn.abort` 请求。

manager 路由使用 `agent: null`，后端根据工作区 manager 解析目标；普通 Agent 使用完整资源地址。
所有命令都通过 `mcl.command` 发送，来源身份由注册连接绑定。命令结果直接展示真实返回值；
执行错误与历史持久化错误分别处理，历史写入失败不会被误报成指令执行失败。

## 状态、历史与流

注册成功后请求 `state.query`；切换工作区或 Agent 时请求 `history.query`。
`state.sync` 提供工作区以及 Agent 的 `status`、`turn`、`sandbox`、排队数量和 pending command。
Sandbox 面板仅显示当前状态，不使用旧 exposed 配置推断或修改 sandbox。
当前协议未发布已加载资源集合时，资源面板会明确提示不可用，不推断默认或动态可见资源。

聊天历史来自后端 `state.sync.histories` 或 `history.result`，按持久记录 key、sequence 和 JSON payload
渲染消息、工具执行失败、嵌套 Inject 和指令记录。Token usage 从历史 payload 汇总，
不依赖旧 Agent token 计数副本。后端记录时间为 0 时显示 `Time unavailable`，不伪造记录时间。

推理增量 `agent.message.delta` / `agent.message.reasoning_delta` 仅作临时展示：
UI 匹配状态中的 `agent_instance` 并用 `sequence` 去重，拒绝旧实例的迟到流。
`agent.message.finished` 使用 `error: null` 表示流正常终止，非空 `{kind,message}` 表示失败。
该终态不是 MCL 权威完成结果；命令完成仍以关联的 `mcl.command_result` 为准。
流结束后再次请求历史，后端历史替换临时流内容；终态后的迟到增量不会让流重新出现。

UI 不使用 localStorage、sessionStorage 保存业务状态，也不乐观追加发送中的用户消息。
刷新或重连后的工作区、历史与状态以后端数据为准。Activity 展示日志和协议错误。

## 检查与测试

```sh
npm test
npm run type-check
npm run build
```

`npm test` 使用已有 Vite 将 TypeScript 测试打包为 SSR 模块，再调用 Node 内置 `node:test`。
测试覆盖显式 context path、二维参数、协议形状、递归历史、实例/序列过滤与流终态、历史 usage。
测试输出写到系统临时目录下唯一的 `margatroid-ui-tests-*` 子目录；退出时验证实际路径并清理。
无需新增依赖，也无需启动 GUI 或 daemon。
