# 伪代码格式

```text
板块使用一级标题，一行注明文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
Resource写在lib板块；持有关系在文末使用树形图。
不写独立逻辑段、进度、计划或验证记录。
```

# 设计范围

Margatroid装配使用独立Axum服务；mecs现有接口与实现保持不动，领域已退出其通用网络桥依赖。

# 板块格式

```text
# lib     src/main.rs  进程入口与退出码
# handler src/main.rs  主目录定位及应用运行
```

# lib

## 函数

私有：
```text
main()
    行为：调用run，报告错误并设置进程退出码。
```

# handler

## 函数

私有：
```text
run() -> Result<(), Error>
    行为：定位用户数据根，构造ApplicationOptions；调用compose::build_app并启动App运行循环。监听及原生路由由Margatroid AxumPlugin负责，不安装mecs ServerPlugin。显式受控退出由compose::shutdown_app提供，保持ECS处理能力直到领域及Axum退出；外部强制终止进程不构成真实清理证明。

data_root() -> Result<PathBuf, Error>
    行为：取得HOME下.margatroid，不把项目目录用作状态目录。
```

# 持有关系

```text
进程main
└── App
    ├── mecs Runtime/AsyncRuntime/Closure设施（本轮保持）
    ├── Margatroid AxumHandle（监听、退出及升级工作跟踪）
    ├── MargatroidConfig
    ├── WorkspaceDefinitions（路径和展示缓存）
    ├── ModelRoutes
    └── 各领域运行Entity
```
