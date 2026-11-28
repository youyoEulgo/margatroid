# WebSocket与HTTP接入

DTO层把网络请求接到Margatroid的领域接口，并将领域状态转换为客户端消息。编写客户端可从消息交互开始；装配后端或增加同步来源可从 [dto_plugin](plugin/README.md) 开始。

## 一次请求如何完成

客户端连接 `/ws` 后，先发送注册消息：

```json
{
  "type": "connection.register",
  "id": "register-1",
  "message": { "client_type": "webui", "name": "browser" }
}
```

注册完成后，客户端可查询Workspace、Agent、任务和记录，或提交生命周期与MCL操作。DTO解析请求并定位目标，把执行交给相应领域，再通过原请求ID交付响应。各操作的JSON结构由 [protocol](../protocol/README.md) 定义。

| 需求 | 网络入口 | 执行来源 |
| --- | --- | --- |
| 查询运行节点 | `workspace.req`、`agent.req` | Workspace与Agent状态 |
| 启动或停止Agent | `agent.start`、`agent.stop` | Driver工具与创建任务 |
| 执行MCL | `mcl.execute` | Agent的MCL队列 |
| 浏览历史、日志和审计 | `agent.history.req`、`agent.log.req`、`agent.mcl.req` | Agent存储 |
| 查找资源 | `resource.req` | 当前资源目录 |

## 接收状态变化

同步来源持有共享的 `Clients` 槽位，按连接注册身份交付消息。状态更新通过领域探针发布，磁盘内容通过异步监控读取。

| 同步内容 | 关联依据 |
| --- | --- |
| Workspace与Agent列表 | 节点地址与当前来源版本 |
| 回合 | Agent地址和turn |
| 任务 | Agent地址、task_id及保留输出窗口 |
| 推理分片与终态 | generation、sequence和tag |
| 持久消息、MCL、日志 | 实际提交序号 |
| Driver UI与模型配置 | 实际文件来源及读取结果 |

状态来源的关闭会退休对应投递能力，并等待读取任务完成。记录流沿提交序号递增，推理文本沿同一generation累积。同步来源的创建、替换和关闭与领域对象的生命周期关联。

## 文件读取

HTTP的 `GET /{workspace}/{path}` 和对应HEAD请求提供Workspace项目目录内的普通文件。`workspace` 是经过URL编码的完整节点地址：读取 `/demo` 工作区的 `sample.txt` 时，请求路径为 `/%2Fdemo/sample.txt`。入口核对Workspace、路径包含关系和文件类型，再交付文件内容或响应头。Driver UI使用 `agent.ui.req` 获取包含revision和文件列表的资源包。

## 开发导航

- [插件接入与同步绑定](plugin/README.md)
- [网络协议](../protocol/README.md)
- [应用装配](../compose/README.md)
- [实现设计](DESIGN.md)
- [完整WebSocket交互示例](../compose/tests/dto_websocket.rs)
