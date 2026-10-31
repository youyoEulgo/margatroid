# 伪代码格式

```text
板块使用一级标题，一行注明文件；无定义不建空模块。
类型、函数使用二级标题；列出字段、签名和直接行为。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib       agent_driver/core/src/lib.rs    Driver文件读取入口
# types pub agent_driver/core/src/types.rs  清单、预算及本次启动材料
# error     agent_driver/core/src/error.rs  Driver读取错误
```

# lib

## 函数

公开：
```text
read_driver(manifest_path: &Path, limits: &AgentDriverReadLimits) -> Result<AgentDriverInput, AgentDriverError>
    行为：有界读取driver.toml、同目录base.lua和settings.toml；校验清单、UTF-8和默认TOML；返回本次材料，不缓存内容。
```

# types pub

## 类型

公开：
```text
AgentDriverManifest：Driver清单
    schema_version: u32
    dependencies: Vec<String>--尚未加载的文件定位输入
    inference: DriverInferenceConfig

DriverInferenceConfig：模型声明
    model: String
    temperature: Option<f32>
    max_output_tokens: Option<u32>
    top_p: Option<f32>
    stop: Vec<String>

AgentDriverInput：本次启动材料
    manifest: AgentDriverManifest
    program: String
    origin: PathBuf
    default_settings: String

AgentDriverReadLimits：读取预算
    manifest_bytes: usize
    program_bytes: usize
    settings_bytes: usize
```

# error

## 类型

公开：
```text
AgentDriverError：读取错误，公开枚举
    InvalidPath | FileMissing | TooLarge | InvalidUtf8
    InvalidManifest | InvalidDefaultSettings | ReadFailed(String)
    impl Display + std::error::Error
```

# 持有关系

```text
Agent启动闭包
├── 请求携带的Driver定位字符串
├── address::resolve_file返回的临时PathBuf
└── agent_driver_core::read_driver返回的AgentDriverInput
    ├── manifest
    ├── program与origin
    └── default_settings
```
