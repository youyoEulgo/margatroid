# Config

config_core定义MargatroidConfig和配置值，只依赖mecs及通用库。config_plugin读取并校验config.toml，安装Resource，不依赖其他领域core或plugin，不创建System、查询事件或文件监听。

```toml
[server]
bind = "127.0.0.1:3939"
allow = "localhost"

[log]
level = "info"

[outbound]
logs = ["type:cli", "type:webui"]
backend_state = ["type:webui"]
```

ConfigPlugin::open在启动阶段读取，build只发布已校验值。filter若提供须非空，完整替代level；tracing表达式由应用装配日志设施时校验。日志目标决定已产生记录的投递，不改变过滤条件。

allow缺省localhost，支持all或IP/CIDR数组；回环bind不能搭配放行非回环的策略。带Origin的请求要求Host同源。目标支持broadcast、type和name，两组均非空且各自不重复。入口策略不等于客户端身份认证。

模型路由归Inference，Workspace路径与展示缓存归Workspace，Agent settings.toml归Storage。Config不复制这些定义或充当万能配置表。各plugin在边界转换OutboundTarget，不让config_core引用DTO契约。整体重载不在此定义独立流程。
