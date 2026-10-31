# 伪代码格式

```text
板块使用一级标题，一行注明文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
Resource写在lib板块；持有关系在文末使用树形图。
不写独立逻辑段、进度、计划或验证记录。
```

# 板块格式

```text
# lib       config/core/src/lib.rs；config/plugin/src/lib.rs  MargatroidConfig；ConfigPlugin
# handler   config/plugin/src/handler.rs                     文件读取与解析
# types pub config/core/src/types.rs                         入口、日志和出站配置
# error     config/core/src/error.rs                         ConfigError
```

# lib

## 类型

公开，定义于config_core：
```text
MargatroidConfig：全局配置，公开Resource
    server_bind: SocketAddr
    ingress: IngressPolicy
    log_level: LogLevel
    log_filter: Option<String>
    logs: Vec<OutboundTarget>
    backend_state: Vec<OutboundTarget>
    impl Resource for MargatroidConfig
```

公开，定义于config_plugin：
```text
ConfigPlugin：全局配置安装插件
    config: MargatroidConfig
    open(path: &Path) -> Result<Self, ConfigError>
        行为：安装前同步读取并校验config.toml，失败返回错误。
    config(&self) -> &MargatroidConfig
        行为：借用已加载配置，供应用装配监听与日志设施。
    impl Plugin for ConfigPlugin
        build(self, app: &mut App)
            行为：拒绝重复安装，将config插入World；不安装System或事件。
```

# handler

## 函数

私有：
```text
read_config(path: &Path) -> Result<MargatroidConfig, ConfigError>
    行为：有界读取UTF-8 TOML；拒绝未知字段；校验监听地址、入口策略、日志及两组非空且不重复的出站目标。
```

# types pub

## 类型

公开：
```text
IngressPolicy：入口策略，公开枚举
    Localhost
    Any
    Addresses(Vec<CidrBlock>)
    evaluate(&self, peer: SocketAddr, origin: Option<&str>, host: Option<&str>) -> Result<(), String>
        行为：检查对端；无Origin时按非浏览器请求处理，有Origin时核对Host同源，拒绝null或非法authority。

CidrBlock：网络地址段
    network: IpAddr
    prefix: u8
    contains(&self, address: IpAddr) -> bool
        行为：同地址族按前缀匹配。

OutboundTarget：未解析的出站目标，公开枚举
    Broadcast | Type(String) | Name(String)

LogLevel：日志级别，公开枚举
    Off | Error | Warn | Info | Debug | Trace
```

# error

## 类型

公开：
```text
ConfigError：配置错误，公开枚举
    ReadFailed(PathBuf) | TooLarge | DecodeFailed | InvalidBind
    InvalidIngress | InvalidLogLevel | InvalidLogFilter | InvalidTarget | DuplicateTarget
    impl Display + std::error::Error
```

# 持有关系

```text
ConfigPlugin（安装前）
└── MargatroidConfig

World（安装后）
└── MargatroidConfig: Resource
    ├── 监听与入口策略
    ├── 日志过滤配置
    └── 出站目标声明
```
