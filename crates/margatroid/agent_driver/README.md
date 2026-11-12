# AgentDriver

agent_driver_core提供Driver文件格式、读取函数及错误，使用标准库、serde/TOML、notify、SHA-256、MIME及Tokio等通用库。Agent启动闭包直接调用，不需要Driver加载Plugin、System、请求事件或oneshot中转。

## 文件

```text
Driver目录
├── driver.toml
├── base.lua
├── setting.toml
└── ui/
    ├── index.html
    └── assets/
```

三个文件均必需，默认setting.toml允许为空。清单声明schema_version、dependencies字符串数组及inference模型参数，使用Driver自己的类型，不依赖其他领域core。

## 启动读取

Workspace将请求定义中的成员启动参数交给StartAgent。Agent启动闭包用Driver定位字符串调用address::resolve_file取得清单路径，再通过agent_driver_core::read_driver读取文件；同步文件读取在blocking worker执行，不阻塞ECS线程。

Agent持有本次实际加载来源的AgentDriverSource，保存Driver地址及清单规范路径，UI观察以此定位。重启完成旧Driver清理时移除原来源，读取新Driver后写入本次来源；运行中的Lua程序仍按既有VM生命周期执行。

## 设置与消息

Agent数据库不存在就创建；Agent setting.toml不存在就从Driver默认模板创建，已有文件不覆盖、不合并，格式损坏直接报错。初始化由Agent启动流程协调，不由Driver读取函数写Agent数据。

Block只保存消息数组，CREATE BLOCK不写MESSAGE类型，存在则打开且不清空；设置使用CONFIG GET、CONFIG SET ? ?、CONFIG DELETE ?。运行指令使用EFFECT，无EMIT前缀。

工具、提示词和沙箱仍由所属领域维护alias到路径的集合；它们在运行期间反复访问，与仅启动时读取一次的Driver不同。

## Driver UI

Driver UI读取及notify观察已实现。agent.ui.req/res/sync经DTO使用这些core能力；ui目录缺失的Driver仍正常运行。

ui目录提供原生Web或Vue/React等编译产物。后端读取完整静态文件集合，计算内容revision，所有文件通过base64随WS交付。ui目录缺失表示该Driver没有UI；存在但index.html缺失表示资源错误。前端负责资源URL映射及中央页面加载，平台保留左右侧栏和顶部栏。

UI监控独立于查询，由DTO每帧按运行Agent实际来源维护。notify捕获变化，100ms安静窗口合并编译写入，双重扫描确认稳定后发布完整集合。core仅提供通用文件观察和owned状态，DTO负责网络类型、32MiB信封预算及ClientSender筛选。初始化、错误及恢复都显式形成状态。

查询读取已发布观察状态，不启动监控或改写Agent。观察退出等待真实文件worker结束，daemon关停通过现有AxumLifetime跟踪完整监控Future。UI变化只影响资源交付，Agent程序是否重启按领域生命周期处理。

当前同步调度为：DriverUiWatch仅在稳定初值或真实内容/错误变化时调用DTO回调，DTO回调产生WebSocketMessageSend，World中的同步System接收并查询实际ClientSender。相同内容及相同错误不产生事件，core不调用RuntimeHandle或提交World闭包。该调度调整已写入源码。
