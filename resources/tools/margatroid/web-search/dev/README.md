# DeepSeek web_search

模型入参沿用 DSH：`{"queries":["...", "..."]}`，1–10条，每条1–128个Unicode字符且不能全为空白。精确重复query去重，保留首次顺序；不裁剪或改写搜索语法。

每次调用先对可信的 `context.agent_info.address` 执行 `CONFIG GET`，只读取该Agent的 `web.deepseek.api_key`。未填写则返回明确失败，不请求网络；不回退环境变量、模型路由、父Agent或全局密钥。不缓存配置，不将配置、认证头或上游原始错误正文返回给模型。

```toml
[web.deepseek]
api_key = ""
```

assistant资源配置模板已包含空字段及web_search/web_fetch导入。已有Agent持久配置不会被模板覆盖：在该Agent的配置中填写同名字段，并在其初始化指令中导入 `tool:margatroid/web-search:dev AS web_search`。子代理有自己的配置，不能自动继承父Agent密钥。

固定调用 `POST https://api.deepseek.com/v1/tools/search`，Bearer鉴权，每个唯一query对应一次HTTP请求，固定 `count=10`、`need_content=false`。API Key和供应商参数不进入模型Schema；不额外读取用户对话推导ori_question，不自动设置localization。与DSH参考的模型Schema及轮转合并一致，当前Lua按顺序发送，避免超出共享HTTP的8并发预算；整个工具120秒，每次请求最多30秒。某条失败则整次调用失败，已完成请求可能已计费，不自动重试。

按结果排名轮转合并各查询、按精确URL去重，最多交付10个来源。summary优先、为空则snippet，保留日期；不自行过滤分数或裁剪摘要。超过10个唯一来源明确标出截断，空结果是正常成功。输出声明网页文本是不可信数据，并要求引用来源链接；网页内容不能作为工具或模型的新指令。

HTTP401提示检查密钥、402提示余额、429提示稍后重试，其余状态显示状态码与固定安全说明；传输异常/错误JSON/非法结果结构返回失败，不转储响应。使用现有 `task:return_result` 和 `task:finish`，不引入宿主特殊API。

网络仍受当前Agent所选沙箱控制。内置workspace-write/read-only策略默认禁止网络，真实调用前须选择允许联网的策略；此脚本不会修改沙箱或绕过网络限制。HTTP宿主默认不使用环境代理、不跟随跳转。当前仅做离线模拟，不向DeepSeek发请求，不消耗额度。

参考：
- https://api-docs.deepseek.com/zh-cn/api/search/
- https://api-docs.deepseek.com/zh-cn/guides/search/
- DSH `packages/web/tool-web/src/search.ts`（queries、去重及轮转合并）
