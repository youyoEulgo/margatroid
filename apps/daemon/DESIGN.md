# MargatroidDaemon

## 函数

私有：
```text
main()
    daemon入口：调用run并将错误转换为进程退出码；不解析任何启动参数

run() -> Result<(), Error>
    组合并启动应用：私有函数，打开主目录中的插件资源，按依赖顺序安装Plugin并运行App
    行为：
        从HOME固定得到~/.margatroid并创建主目录
        检查models.toml和config.toml存在
        加载并验证config.toml中的server.bind和全部出站配置
        使用server.bind构造ServerPlugin
        将全局只读WebSocket目标配置交给DtoPlugin和InferencePlugin
        打开AgentImage、Workspace、Skill、Workflow和Lua Plugin所需目录
        安装运行时、日志、Server和全部领域Plugin
        安装DtoPlugin和ConnectionPlugin
        记录启动信息并调用AppRunExt::run

data_root() -> Result<PathBuf, Error>
    构造固定主目录：私有函数，返回HOME下的.margatroid；HOME缺失时启动失败
```

## 逻辑

```text
main
    -> run
        -> 打开~/.margatroid/config.toml
        -> 使用server.bind安装ServerPlugin
        -> 按依赖顺序安装ToolPlugin和LuaPlugin等全部Plugin
        -> AppRunExt::run

边界：
    daemon负责固定主目录定位、Plugin配置与装配、进程退出
    daemon不接受启动参数；运行配置以~/.margatroid/config.toml为准
    daemon不定义或注册业务System
    daemon不解析API消息，不路由Workspace或Agent，不构造前端状态，不转发日志
    API DTO与领域命令转换、领域状态和日志的客户端投影均由DtoPlugin负责
```
