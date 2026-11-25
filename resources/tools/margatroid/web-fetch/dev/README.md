# web_fetch

DeepSeek 网页抓取工具，模型入参参照 DSH，仅接受必填 `url`。URL 为不带用户名或密码、包含 host 的 HTTP/HTTPS 地址，最多8192个 Unicode 字符。脚本做基础形状检查，DeepSeek 服务端再执行完整 URL 和目标安全校验。

每次执行从真实调用方 `context.agent_info.address` 通过 `CONFIG GET` 读取 `[web.deepseek]` 下的 `api_key`。不读取父 Agent、全局模型设置、环境变量或其他备用密钥；缺失或空白凭据明确失败。工具参数不接受 API Key 或自定义 API 地址。

使用 `margatroid.http:request` 调用固定的 `POST https://api.deepseek.com/v1/tools/fetch`，携带 Bearer 鉴权和 JSON 请求 `{url}`。请求超时30秒，响应最多8MiB，工具总超时60秒。不自动重试、跟随重定向或直接抓取目标网页；实际目标读取由 DeepSeek API 完成。内置沙箱策略可能禁止网络，使用时必须选择允许该 API 联网的策略；本资源不修改策略或绕过 worker 沙箱。

成功要求 HTTP 200、JSON 中的 `title` 和 `content` 均为字符串且正文非空白。输出保留完整 Markdown、不截断，附带原 URL、标题和外部不可信内容说明。非200状态、传输失败和非法响应返回固定安全错误，不回显密钥、请求、原始上游错误体或底层错误信息。

脚本通过 `task:return_result({ok,content})` 交付工具结果，然后 `task:finish(context.task_id)` 收口。未配置凭据的资源安装不会自动发起请求；测试应使用本地桩，不连接用户真实服务或发起付费请求。DeepSeek 独立 Fetch 按其官方价格计费。

参考：https://api-docs.deepseek.com/zh-cn/api/fetch/
