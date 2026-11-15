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
        app = compose::build_app(options)；克隆必需RuntimeLogStore，启动目录、实际监听成功、关闭请求及关闭完成先作为完整报告提交，再镜像终端输出
        创建信号监听运行时，安装listen_for_shutdown
        app.run_until(stopping.load)
        await compose::shutdown_app(&mut app)，持续推进ECS及真实清理
        清理未确认: persist_failure保留原错误及回溯，终端报错并继续监督
        清理完成: 结束信号监听，释放App；仍持Store克隆，最后完成报告也实际提交；Axum最终错误同样持久记录再返回退出码

listen_for_shutdown(stopping: Arc<AtomicBool>, handle: RuntimeHandle, logs: RuntimeLogStore) -> Result<JoinHandle<()>, IoError>
    行为：
        Unix监听SIGINT、SIGTERM；非Unix监听Ctrl+C
        每次收到信号调用request_shutdown；监听保留至清理完成

request_shutdown(stopping: &AtomicBool, handle: &RuntimeHandle, logs: &RuntimeLogStore)
    行为：
        首次信号: stopping = true; handle.wake()
        重复信号: 输出清理仍在进行，继续等待清理

persist_notice(store: &RuntimeLogStore, stage: &str, message: &str, details: Value)
    行为：track_caller取得真实文件/行号，提交Info级生命周期报告及进程上下文；必需探针由Store提交入口触发，不用tracing级别过滤隐藏退出记录。
persist_failure(store: &RuntimeLogStore, stage: &str, error: &(impl Debug + Display))
    行为：提交Error级完整Debug/Display诊断、来源及force_capture的Rust回溯，提交失败显式报告，不冒充安全退出。

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
