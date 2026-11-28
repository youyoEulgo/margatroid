# inference_plugin

`inference_plugin` 将 [inference_core](../core/README.md) 的一次推理请求接到模型 HTTP/SSE，并向 Client 投递累计文本与最终消息。接入者提供模型文件、mecs 运行设施和具备相应组件的 Agent。

## 核心对象

| 对象 | 职责 |
| --- | --- |
| `InferencePlugin` | `open(&Path)` 校验模型文件，`with_schedule` 选择运行阶段 |
| `ModelConfigFile` | 每次执行重读路由，提供读取和原子替换闭包 |
| `ApiAdapter` | 按 provider/API 组合选择具体请求处理器及解码器 |
| `ProviderFragment` | 正文、思考、工具参数、usage 和终态的单一通道材料 |
| `StreamGate` / `ResponseAccumulator` | 接受分片、冻结结果、生成累计快照 |
| `StreamDelivery` | generation、tag、来源租约及 Client 投递 |

后五项中的适配与流实现属于插件内部；调用方通过 core 请求、结果和句柄协作。

## 装配与模型文件

安装顺序及公开调用如下，执行环境使用 Tokio，模型文件路径由应用提供。

```rust
use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin};
use core_plugin::App;
use inference_plugin::InferencePlugin;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_plugin(InferencePlugin::open(Path::new("models.toml"))?);
    Ok(())
}
```

默认 schedule 为 `UPDATE`，支持 `UPDATE`、`POST_UPDATE`，二者均在 `PRE_UPDATE` 中止控制之后执行。插件按一次安装配置并检查运行资源与闭包入口。

```toml
[[models]]
id = "primary"
model = "gpt-4.1"
provider = "openai"
api_type = "responses"
base_url = "https://api.openai.com/v1"
api_key = "application-supplied-key"
context_window = "1m"
```

provider/API 的七种组合与消息表达条件见 [领域矩阵](../README.md)。`context_window` 支持 k/m/b/t 后缀，默认 1m；可选字段包含 `thinking` 和 `reasoning_effort`。文件读取上限为 1 MiB；配置错误、路由 id 缺失及组合错误以 `InferenceError` 返回。

## 一次调用的执行路径

1. 按 [HTTP 测试](../../compose/tests/inference_http.rs) 准备 Agent、Address、`ActiveInference` 与 Tools，发送 `InferenceRequest` 并保留 oneshot receiver。
2. 受理时核对 Entity、Agent 状态、显式 turn 与当前推理，固定消息、工具定义、实例和投递来源。
3. 监督任务重读模型文件，构造 `DomainRequest`，经具体 `TryFrom` 和厂商特化生成 HTTP 请求。
4. HTTP worker 向容量 32 的通道写入分片，消费端在接受门内更新唯一累积器，锁外投递文本快照。
5. 正常完成先等待HTTP worker join，再交付oneshot与一次full；已知错误先交付失败，监督继续等待清理。调用方通过 `InferenceHandle::wait_finished()` 等待监督任务结束。

完整测试使用本地 HTTP 服务捕获端点、认证头及请求体，再核对回复和任务结束，适合复制其装配与事件调用方式。

`agent.message.chunk.sync` 携带相同 generation 下的递增 sequence、完整累计 `content` 与 `reasoning`。`agent.message.full.sync` 携带最终内部消息、`history_sequence=null` 和 nullable error。两种出口均携带执行 tag：普通推理为 `inference`，压缩为 `compact`，省略时为 `null`；有效 tag 为最多 512 字节、含可见字符且控制字符数量为零的字符串。

## 错误、取消与预算

取消同步冻结接受门，普通取消返回已接受正文及思考的 `completed`，工具调用与 usage 为空。来源替换使用 `retire_source()`，任务收口使用 `wait_finished()`；清理超预算返回 `CleanupFailed`，监督继续等待真实 join。

HTTP 413 或明确的请求体超限机器 code/type 映射为 `RequestBodyTooLarge`，details 保留 HTTP 状态及可用关联字段。诊断文本经认证信息清理及有界读取后交付。协议终态、工具 ID/名称/参数完整性、图片格式及大小均按对应入口校验。

整体默认期限 180 秒，连接超时 30 秒，流开始后的读取空闲上限 60 秒；响应总量 128 MiB、单 SSE 事件 4 MiB。首响应等待采用整体剩余期限。

## 源码与测试导航

- [安装入口](src/lib.rs)、[事件系统](src/system.rs)、[受理与监督](src/handler.rs)、[具体转换及解码](src/types.rs)。
- [适配测试](tests/internal/adapters.rs)：七种处理器、跨格式消息与 reasoning-only。
- [流测试](tests/internal/stream.rs)：真实 HTTP、取消、真实 join、累计快照和 tag。
- [工具定义测试](tests/internal/tool_contract.rs)：当前 Tools 快照及完整大目录。
- [工具结果校验](tests/internal/call_validation.rs)：结构完整性及安全诊断。
