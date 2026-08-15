# ShellPlugin

ShellPlugin把`shell:<scope>/<name>:<tag>`资源注册为Agent工具，并通过异步子进程执行资源的
`main.sh`。资源工具对模型可见，`tool:builtin/shell:latest`执行器对模型隐藏。

Shell资源包：

```text
shells/<scope>/<name>/<tag>/
├── shell.toml
├── input.schema.json
└── main.sh
```

资源脚本接收模型提供的`command`作为第一个参数，工作目录为Agent项目根目录。完整接口见
`DESIGN.md`。`examples/shells/local/sh/latest/`提供基础的POSIX Shell资源示例。
