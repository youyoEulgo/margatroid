# ShellPlugin

ShellPlugin把`shell:<scope>/<name>:<tag>`资源注册为Agent工具。默认资源通过异步子进程执行
`main.sh`；`shell.toml`设置`persistent = true`后，资源改用每个Agent独立的长驻Bash会话，
通过Unix PTY保留cwd、环境变量和shell变量，并串行执行同一Agent的命令；PTY模式下stdout和stderr
按终端语义合并。资源工具对模型可见，
`tool:builtin/shell:latest`执行器对模型隐藏。

Shell资源包：

```text
shells/<scope>/<name>/<tag>/
├── shell.toml
├── input.schema.json
└── main.sh
```

资源脚本接收模型提供的`command`作为第一个参数，工作目录为Agent项目根目录。完整接口见
`DESIGN.md`。`examples/shells/local/bash/latest/`提供基础的Bash资源示例。
