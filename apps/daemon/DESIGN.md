# MargatroidDaemon

## 类型

私有：
```text
DaemonConfig：daemon启动配置，私有结构体
    bind: SocketAddr--ServerPlugin监听地址
    data_root: PathBuf--Margatroid主目录
```

## 函数

私有：
```text
main()
    daemon入口：解析启动配置，调用run并将错误转换为进程退出码

run(config: DaemonConfig) -> Result<(), Error>
    组合并启动应用：私有函数，打开主目录中的插件资源，按依赖顺序安装Plugin并运行App
    行为：
        创建并规范化data_root
        检查models.toml存在
        打开AgentImage、Workspace、Skill和Workflow Plugin所需目录
        安装运行时、日志、Server和全部领域Plugin
        安装DtoPlugin、ConnectionPlugin和ApiIntegrationPlugin
        记录启动信息并调用AppRunExt::run

parse_args(arguments: impl IntoIterator<Item = String>) -> Result<DaemonConfig, String>
    解析启动参数：私有函数，读取bind与data-root配置

parse_bind(value: String) -> Result<SocketAddr, String>
    解析监听地址：私有函数，将字符串转换为SocketAddr

default_data_root() -> PathBuf
    构造默认主目录：私有函数，返回HOME下的.margatroid，HOME缺失时使用当前目录

absolute_path(path: &Path) -> Result<PathBuf, Error>
    构造绝对路径：私有函数，绝对路径原样返回，相对路径拼接当前目录

usage() -> &'static str
    返回帮助文本：私有函数，描述当前启动参数
```

## 逻辑

```text
main
    -> parse_args
    -> run
        -> 打开主目录资源
        -> 按依赖顺序安装Plugin
        -> AppRunExt::run

边界：
    daemon负责进程参数、路径准备、Plugin配置与装配、进程退出
    daemon不定义或注册业务System
    daemon不解析API消息，不路由Workspace或Agent，不构造前端状态，不转发日志
    API DTO与领域命令转换由DtoPlugin负责，领域状态到客户端事件的投影由ApiIntegrationPlugin负责
```
