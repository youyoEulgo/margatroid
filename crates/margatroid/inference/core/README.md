# inference_core

`inference_core` 提供厂商通用的推理消息、请求、结果与生命周期控制类型，适合 Rust 调用方、领域桥接和模型配置观察者接入。包依赖为 mecs `core_plugin`、Serde 与 Tokio；HTTP 执行由 [inference_plugin](../plugin/README.md) 装配。

## 核心对象

| 对象 | 用途 |
| --- | --- |
| `InferenceMessage` / `InferenceImage` | 结构化角色消息、思考、工具关联及图片 |
| `InferenceParameters` / `DomainRequest` | 路由选择键、采样参数与完整模型输入 |
| `InferenceRequest` / `InferenceReply<T>` | 执行身份、可选 turn/tag、取消及 oneshot 回复 |
| `InferenceResponse` | 正文、思考、工具调用、usage 和停止原因 |
| `ActiveInference` / `InferenceHandle` | Agent 当前推理与共享任务收口 |
| `ModelRoutes` / `AgentInferenceConfig` | 路由集合、Agent 参数及安全公共投影 |
| `ModelConfigFile` / `ModelObserverFactory` | 文件操作闭包及按 owner 绑定的变化回调 |

`InitInference` 为 Agent 挂载默认 `ActiveInference`，已有组件沿用当前值。`InferenceRequest.config.model` 对应路由 id，`ModelRoute.model` 对应上游模型名。

## 构造一份领域输入

下面使用公开类型完成输入构造、公共参数校验及序列化；具体格式转换由插件接续。

```rust
use inference_core::{DomainRequest, InferenceMessage, InferenceParameters};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = InferenceParameters {
        model: "primary".into(),
        temperature: None,
        max_output_tokens: Some(128),
        top_p: None,
        stop: vec![],
    };
    config.validate()?;
    let input = DomainRequest {
        messages: vec![InferenceMessage::User {
            content: "请回答 hello".into(),
            images: vec![],
        }],
        tools: vec![],
        config,
    };
    let encoded = serde_json::to_value(&input)?;
    assert_eq!(encoded["messages"][0]["type"], "user");
    Ok(())
}
```

完整执行时，调用方将上述消息与参数放入 `InferenceRequest`，同时提供 `id`、`agent`、`turn`、`tag`、`cancel` 和 `reply`。`reply` 类型为 `Arc<Mutex<Option<oneshot::Sender<Result<InferenceResponse, InferenceError>>>>>`。事件发送与应用驱动的完整写法见 [HTTP 接入测试](../../compose/tests/inference_http.rs)。

`DomainRequest` 的三个字段分别为 `messages`、`tools`、`config`。执行身份和 tag 由 `InferenceRequest` 承载；厂商路由及认证材料由 `ModelRoute` 承载。消息序列化时，用户与工具图片集合使用 `data`，工具参数使用 JSON 值。

## 结果与任务控制

`InferenceResponse` 的 `stop_reason` 为 `Completed`、`ToolCalls`、`Length`、`ContentFilter` 或 `Other(String)`。`empty_completed()` 返回空正文、空思考、空工具调用与空 usage 的普通完成结果。

`InferenceHandle::cancel()` 同步调用 `stop_stream` 后触发取消令牌；`wait_finished().await` 等待共享任务的真实结束，支持多个等待者。`retire_source()` 用于 Agent 关闭或来源替换时淘汰来源帧。`ActiveInference.current` 在受理与清理期间持有本次句柄。

`AgentInferenceConfig::set(parameters, routes)` 更新参数并发布当前模型投影；`ModelRoutes::replace(entries, configs)` 刷新路由并逐一发布。`PublicModelInfo` 包含模型标识、provider、API、上下文窗口和思考选项；认证材料留在路由执行侧。

## 源码与测试导航

- [公开导出与模型资源](src/lib.rs)：路由、观察工厂和文件闭包接口。
- [请求事件](src/events.rs)：`InitInference`、`InferenceRequest` 及回复别名。
- [消息、结果与句柄](src/types.rs)：参数校验、序列化和任务等待。
- [Agent 组件](src/components.rs)：当前推理及模型公共投影。
- [错误类型](src/error.rs)：错误分类、说明与结构化 details。
- [真实 HTTP 调用](../../compose/tests/inference_http.rs) 与 [句柄收口测试](../plugin/tests/internal/stream.rs)。
