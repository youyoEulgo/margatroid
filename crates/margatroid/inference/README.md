# Inference

Inference 为 Agent 执行一次模型推理，将完整消息上下文转换为选定 API 请求，并返回统一的正文、思考、工具调用和用量。开发者可从本页选择接入层、模型组合与消息表达规则。

## 核心对象与接入层

| 对象 | 职责 | 入口 |
| --- | --- | --- |
| `InferenceMessage` | System、User、Assistant、Tool 领域消息 | [core 类型](core/src/types.rs) |
| `DomainRequest` | 消息、工具定义和公共采样参数 | [core README](core/README.md) |
| `InferenceRequest` | Agent、turn、tag、取消和唯一回复 | [事件](core/src/events.rs) |
| `InferenceHandle` | 冻结文本、取消和等待任务收口 | [core 类型](core/src/types.rs) |
| `ModelRoutes` / `ModelConfigFile` | 模型路由与配置文件操作 | [core 入口](core/src/lib.rs) |
| `InferencePlugin` | 路由重读、HTTP、SSE 和客户端投递 | [plugin README](plugin/README.md) |

core 依赖 mecs 与通用库；plugin 组合 Agent、Client、Tools 和 address 的公开契约。MCL 负责领域消息转换，Driver 负责显式记录历史，工具领域负责执行模型返回的调用。

## provider 与 API 矩阵

| provider | api_type | reasoning-only assistant 的请求投影 |
| --- | --- | --- |
| `openai` | `chat_completions` | 返回 `InvalidRequest` |
| `openai` | `responses` | assistant 的 `content` 投影为 `""` |
| `anthropic` | `messages` | 返回 `InvalidRequest` |
| `deepseek` | `chat_completions` | 由 thinking 选项及工具调用决定 `reasoning_content` |
| `deepseek` | `messages` | 返回 `InvalidRequest` |
| `deepseek` | `responses` | 本次 HTTP input 仅保留可表达项 |
| `openrouter` | `chat_completions` | 保留通用 Chat 请求中的 assistant 表达 |

`provider` 与 `api_type` 由配置显式指定；矩阵之外的组合返回路由错误。Messages 要求 `max_output_tokens`；Responses 要求 `stop` 为空，并以 `store=false` 提交完整本地上下文。

## 消息表达与转换

转换依次经过三层：`DomainRequest` / `InferenceMessage` → 具体请求类型的 `TryFrom<DomainRequest>` → `OpenAi<T>`、`Anthropic<T>`、`DeepSeek<T>`、`OpenRouter<T>` 的具体 `process`。`T` 分别为 `ChatCompletionsRequest`、`MessagesRequest`、`ResponsesRequest`；`ApiAdapter` 负责组合分派及配套解码器选择。

| 输入 | 表达与条件 |
| --- | --- |
| 正文及工具关联 | 保留角色、调用 ID、工具名称、JSON 参数及结果配对 |
| `reasoning` | 领域消息保留一份原值，历史映射在厂商特化层完成 |
| reasoning-only | 通用转换保留身份，最终投影采用上表规则 |
| 用户与工具图片 | JSON 集合键 `data`，每项含 `media_type` 和 base64 `data` |
| 图片格式 | PNG/JPEG/GIF/WebP；每条最多 8 张、每张 5 MiB、合计 20 MiB |
| 工具结果图片 | Messages/Responses 使用原生布局；Chat 在整批结果后追加 user 图片投影 |

切换模型时，以同一份领域消息重新构造目标请求。可读思考统一输出为 `reasoning`；签名及加密协议材料由适配边界消费。正文或工具调用齐备的 assistant 保留相应内容。

## 一次完整推理

1. 从模型文件 `[[models]]` 配置选择 id，通过 `InferencePlugin::open` 安装推理处理。
2. 调用方发送含完整 Agent Entity、消息、参数及 oneshot 的 `InferenceRequest`；当前 Agent 的 Tools 固定为本次完整工具定义快照。
3. 监督任务重读模型文件、按三层转换构造 HTTP，请求与响应通过对应 API 编解码器处理。
4. `ProviderFragment` 经过接受门进入唯一累积器；文本变化发出累计 `content` / `reasoning` 快照，最终通过 oneshot 返回 `InferenceResponse`。
5. MCL 将结果转换为内部消息，Driver 按需调用 `history_append`；调用方通过句柄 `wait_finished()` 等待真实任务结束。

可运行的完整例子见 [真实 HTTP 测试](../compose/tests/inference_http.rs)，其中包含配置文件、Agent、请求、应用 tick、回复及 HTTP 捕获。

`turn=None` 表示自由调用；`Some(turn)` 要求 Running Agent 与当前回合匹配。tag 在本次 chunk/full 顶层标识执行用途，普通推理使用 `inference`，压缩使用 `compact`，省略值输出 `null`。tag 作用域为本次投递元数据。

取消先关闭接受门并冻结已接受文本，再请求 worker 退出；普通取消返回 `completed`，工具调用与 usage 为空。`wait_finished()` 对应监督任务真实 join；清理超预算返回 `CleanupFailed` 并保留监督。

## 接口与测试导航

- [core 公共契约](core/README.md)：请求、模型配置、结果与控制句柄。
- [plugin 装配](plugin/README.md)：运行依赖、模型文件和流出口。
- [具体转换测试](plugin/tests/internal/adapters.rs)：七种组合、reasoning 和媒体投影。
- [流与取消测试](plugin/tests/internal/stream.rs)：累计快照、tag、协议终态、错误与清理。
- [工具定义测试](plugin/tests/internal/tool_contract.rs)：受理快照及大工具目录。
