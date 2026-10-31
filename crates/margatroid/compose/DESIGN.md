# 伪代码格式

```text
板块使用一级标题，一行注明文件，无内容不建空模块。
类型、函数使用二级标题，列出字段、签名及直接行为。
Resource写在lib板块；持有关系在文末使用树形图。
不写独立逻辑段、进度、计划或验证记录。
```

# 板块格式

```text
# lib     src/lib.rs      应用装配入口
# handler src/handler.rs  插件构造与安装
# types   src/types.rs    应用路径及装配参数
# error   src/error.rs    装配错误
```

# lib

## 函数

公开：
```text
build_app(options: ApplicationOptions) -> Result<App, ComposeError>
    行为：构造插件、读取启动配置、安装基础设施和业务插件，返回未进入运行循环的App。
```

# handler

## 函数

私有：
```text
prepare(options: &ApplicationOptions) -> Result<ApplicationParts, ComposeError>
    行为：调用ConfigPlugin::open、WorkspacePlugin::open和InferencePlugin::open；准备资源根及runner路径；任一失败返回，不自动启动工作区。

install(app: &mut App, parts: ApplicationParts) -> Result<(), ComposeError>
    行为：先安装mecs运行、异步、闭包、日志与Server设施，再安装业务Plugin及HTTP路由；各生命周期按Stop、Restart、Start顺序挂载，最后接入DTO。

```

# types

## 类型

公开：
```text
ApplicationOptions：应用装配参数
    data_root: PathBuf
    runner_path: PathBuf
```

私有：
```text
ApplicationParts：安装前材料--已构造插件、监听参数及日志过滤
```

# error

## 类型

公开：
```text
ComposeError：装配错误--保留失败领域、文件路径及原错误，不输出凭据
    impl Display + std::error::Error
```

# 持有关系

```text
Daemon
└── build_app
    ├── ApplicationOptions
    ├── mecs基础设施
    └── 各领域Plugin
        └── 各自core
```
