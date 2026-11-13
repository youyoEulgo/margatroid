# 伪代码格式

```text
板块使用一级标题，一行注明文件；无定义不建空模块。
类型、函数使用二级标题；列出字段、签名和直接行为。
不写独立逻辑段；持有关系使用树形图。
```

# 板块格式

```text
# lib       agent_driver/core/src/lib.rs、ui.rs    Driver读取及UI资源观察入口
# types pub agent_driver/core/src/types.rs、ui.rs  清单、预算、UI资源及本次启动材料
# error     agent_driver/core/src/error.rs  Driver读取错误
```

# lib

## 函数

公开：
```text
read_driver(manifest_path: &Path, limits: &AgentDriverReadLimits) -> Result<AgentDriverInput, AgentDriverError>
    行为：有界读取driver.toml、同目录base.lua和setting.toml；校验清单、UTF-8和默认TOML；返回本次材料，不缓存内容。

read_driver_ui(manifest_path: &Path, limits: DriverUiReadLimits) -> Result<Option<Arc<DriverUiBundle>>, DriverUiError>
    行为：
        从实际清单父目录定位ui；目录缺失返回None，存在时要求index.html。
        在blocking worker内递归读取普通文件，以规范化相对路径排序；ui根规范路径必须位于实际Driver目录内，所有最终文件路径限制在该ui根内。
        读取前后复核文件集合及内容；变化时返回Unstable，由监控继续合并重读。
        raw_bytes为原始数据及路径和媒体类型的累计读取预算，上限32MiB，超出返回TooLarge；媒体类型由路径确定，未知类型使用application/octet-stream。
        对规范排序的路径、媒体类型和原始字节计算SHA-256，返回完整owned集合。

open_driver_ui_watch(manifest_path: PathBuf, on_change: Arc<dyn Fn(DriverUiState) + Send + Sync>) -> (DriverUiWatch, DriverUiMonitor)
    行为：Agent实际Driver来源安装时构造独立取消能力及Initializing状态；注入的UI同步上下文绑定该次来源，DriverUiMonitor在已有Agent异步启动流中运行，不由查询或活动帧扫描启动。

run_driver_ui_monitor(context: DriverUiMonitor) -> Future<Output = Result<(), DriverUiError>>
    行为：
        在blocking worker初始化notify监控，观察ui及可识别其创建、删除和替换的Driver祖先目录；过滤本Driver ui之外的变化。
        初始化立即读取一次；后续新增、修改、删除使用100ms安静窗口合并，再调用read_driver_ui；完整读取及新状态publish同在blocking worker内执行。
        Unstable保留待处理状态并重读；其他读取错误发布Failed并保持后续观察，路径恢复后重新发布Ready。
        读取或监控初始化错误进入有间隔的恢复尝试，取消后等待正在执行的blocking worker真实返回。
        更新状态时在短锁内同时递增内部版本，释放锁后调用on_change；初次稳定结果及后续内容或错误状态变化才通知，相同Ready内容或相同错误保持等待。
        on_change经注入的UI SyncContext直接选择当前Clients投递StateKey，编码资源在对应来源的稳定缓存中只生成一次；队列不保存多份旧集合。
        当前source失效后旧读取不得发布到新Agent来源；退出等待真实读取及notify收口。
        取消后释放notify句柄并返回，完整监控Future退出才算清理结束。

DriverUiWatch::state(&self) -> DriverUiState
    行为：短锁克隆当前状态，供纯查询读取。
DriverUiWatch::cancel(&self)
    行为：触发本监控取消能力。
DriverUiWatch::is_cancelled(&self) -> bool
    行为：读取取消状态，供DTO在异常终结后重新建立观察。
DriverUiWatch::publish(&self, state: DriverUiState)
    行为：发布稳定读取结果，锁外交付on_change，供注入的DriverUiSyncBinding构造网络投影并通过同一Clients共享槽位直接投递，不发ECS事件、不调用World或RuntimeHandle。
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

DriverUiReadLimits
    raw_bytes: usize
DriverUiBundle
    revision: String
    entry: String
    files: Vec<DriverUiFile>
    impl Clone + Debug + PartialEq + Eq
DriverUiFile
    path: String
    media_type: String
    data: Vec<u8>
    impl Clone + Debug + PartialEq + Eq
DriverUiState
    Initializing
    Ready(Option<Arc<DriverUiBundle>>)
    Failed(DriverUiError)
    impl Clone + Debug + PartialEq + Eq
DriverUiWatch
    shared: Arc<DriverUiShared>
    impl Clone
    state(&self) -> DriverUiState
    cancel(&self)
    is_cancelled(&self) -> bool
    publish(&self, state: DriverUiState)
DriverUiShared
    state: RwLock<(u64, DriverUiState)>--状态及内部单调版本同锁提交
    cancel: CancellationToken
    on_change: Arc<dyn Fn(DriverUiState) + Send + Sync>
DriverUiMonitor
    manifest_path: PathBuf
    limits: DriverUiReadLimits
    watch: DriverUiWatch

DriverUiSyncBinding
    bind: Arc<dyn Fn(Entity, String, PathBuf) -> DriverUiSyncContext + Send + Sync>
    行为：先建立独立UI观察视图及回调，再把on_change传入open_driver_ui_watch；encode由DTO提供，内容revision继续只作为摘要身份。
DriverUiSyncContext
    state: Arc<RwLock<(u64, DriverUiState)>>--观察的派生只读视图，无World
    on_change: Arc<dyn Fn(DriverUiState) + Send + Sync>
    sync: SyncContext<DriverUiState>
    行为：on_change持有视图并提交稳定变化，source.prepare只捕获视图Weak；视图本身不持有sync或watch，避免回环。

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

DriverUiError
    Initializing | EntryMissing | ReadFailed(String) | PathInvalid | TooLarge | Unstable
    impl Clone + Debug + PartialEq + Eq + Display + std::error::Error
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

DTO按Agent持有的实际Driver清单路径建立UI观察
├── DriverUiWatch
│   └── Arc<DriverUiShared>
│       ├── 稳定完整资源集合或错误
│       ├── CancellationToken
│       └── DTO的on_change回调
└── run_driver_ui_monitor
    ├── notify文件监控
    └── blocking读取任务
```
