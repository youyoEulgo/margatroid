# Margatroid

Margatroid 是一个 Agent 运行框架。它提供 Workspace、Agent 生命周期、消息与存储、模型推理、工具进程、沙箱以及 WebSocket API；具体 Agent 如何组织上下文、推进回合、调用工具和压缩会话，由 Lua 驱动。

## 构建

要求安装Rust工具链（Cargo/rustc）。

```bash
git clone https://github.com/youyoEulgo/margatroid.git
cd margatroid
./install.sh
./target/release/margatroid
```

Windows PowerShell：

```powershell
git clone https://github.com/youyoEulgo/margatroid.git
cd margatroid
.\install.ps1
./target/release/margatroid.exe
```

安装脚本会编译并安装程序、部署资源及辅助二进制、生成首次配置；已有配置默认保留。模型配置中的API地址、模型和密钥需自行填写。

### 数据目录

应用将数据保存在用户主目录的 `.margatroid` 中（Unix读取HOME，Windows读取USERPROFILE），会创建主目录和下列初始子目录；配置和资源由安装脚本部署。

```text
~/.margatroid/
├── config.toml
├── models.toml
├── workspaces/
├── agent-drivers/
├── prompts/
├── sandboxes/
└── tools/
```

四类资源目录对应仓库 `resources/` 中的同名目录。

服务配置示例，端口仅为示例值：

```toml
[server]
bind = "127.0.0.1:3939"
```

`models.toml` 至少配置一个有效路由。以下仅展示格式，使用前替换地址、模型和凭据：

```toml
[[models]]
id = "default"
provider = "openai"
api_type = "responses"
base_url = "https://api.openai.com/v1"
model = "YOUR_MODEL"
api_key = "YOUR_API_KEY"
```

### 沙箱

可信 supervisor 在沙箱外管理子进程，选中策略时通过 Landstrip 启动受限 worker。Landstrip按平台使用原生后端：Linux显式选择container，macOS使用Seatbelt，Windows使用其原生隔离实现。当前工具监督与PTY的平台适配情况见[工具运行说明](crates/margatroid/tool/plugin/README.md)。

当前验证版本为 Landstrip 0.22.5

默认从 `PATH` 查找 Landstrip；`MARGATROID_SANDBOX_BACKEND` 可指定其可执行文件路径。

## 资源与扩展

资源标识采用完整的 `type:scope/name:tag`，例如：

```text
driver:margatroid/assistant:dev
tool:margatroid/task-list:dev
prompt:margatroid/assistant-soul:dev
sandbox:margatroid/workspace-write:dev
```

文件型工具通过MCL导入，例如：

```text
IMPORT TOOL tool:margatroid/task-list:dev AS task_list
```

静态工具以 `tool.toml` 为入口，描述取其中的 `description`，同时读取 `input.schema.json` 与 `main.lua`。动态工具描述以注册请求传入的值为准。

Assistant默认初始化保存在资源的 `setting.toml`，每条指令为 `{command, arguments}`，先执行default再执行custom。公共Lua宿主、注册能力及工具生命周期见 [工具说明](crates/margatroid/tool/README.md) 和 [Lua运行时说明](crates/margatroid/lua_runtime/README.md)。

持久Agent布局包含 `state.sqlite`、`setting.toml` 和 `state.sqlite.blocks/<Block名>/`，timeline及业务快照与Agent一起管理。

## 许可证

项目采用 [MIT许可证](LICENSE)，第三方说明见 [NOTICE](NOTICE)。
