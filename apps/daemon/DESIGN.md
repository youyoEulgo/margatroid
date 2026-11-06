# 伪代码格式

```text
板块使用一级标题，一行注明文件，无内容不建空模块。
类型、函数使用二级标题；方法签名下写行为块。
method(arguments) -> ReturnType
    行为：
        执行逻辑
Resource写在lib板块；持有关系在文末使用树形图。
不写独立逻辑段、进度、计划或验证记录。
```

# 板块定义

```text
# lib     src/main.rs  进程入口与退出码
# handler src/main.rs  主目录定位及应用运行
```

# lib

## 函数

私有：
```text
main()
    行为：
        调用run，报告错误并设置进程退出码。
```

# handler

## 函数

私有：
```text
run() -> Result<(), Error>
    行为：
        从HOME定位.margatroid，构造ApplicationOptions
        app = compose::build_app(options)
        创建信号监听运行时，安装listen_for_shutdown
        app.run_until(stopping.load)
        await compose::shutdown_app(&mut app)，持续推进ECS及真实清理
        清理未确认: 报错并继续监督
        清理完成: 结束信号监听，释放App，返回退出码

listen_for_shutdown(stopping: Arc<AtomicBool>, handle: RuntimeHandle) -> Result<JoinHandle<()>, IoError>
    行为：
        Unix监听SIGINT、SIGTERM；非Unix监听Ctrl+C
        每次收到信号调用request_shutdown；监听保留至清理完成

request_shutdown(stopping: &AtomicBool, handle: &RuntimeHandle)
    行为：
        首次信号: stopping = true; handle.wake()
        重复信号: 输出清理仍在进行，继续等待清理

data_root() -> Result<PathBuf, Error>
    行为：
        取得HOME下.margatroid，不把项目目录用作状态目录。
```

# 持有关系

```text
进程main
├── 信号监听任务与stopping标记
└── App
    ├── mecs Runtime/AsyncRuntime/Closure设施
    ├── Margatroid AxumHandle（监听、退出及升级工作跟踪）
    ├── MargatroidConfig
    ├── WorkspaceDefinitions（路径和展示缓存）
    ├── ModelRoutes
    └── 各领域运行Entity
```
