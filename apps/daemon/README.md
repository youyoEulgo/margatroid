# margatroidd

`margatroidd` 是 Margatroid V3 daemon 的 composition root，只负责创建 ECS App、
安装默认 Plugin 组合并运行主循环。

```bash
cargo run -p margatroidd
```

环境变量：

- `--bind <ADDRESS>` / `MARGATROID_BIND`：监听地址，默认 `127.0.0.1:3939`。
- `--data-dir <PATH>` / `MARGATROID_HOME`：daemon 数据目录，默认 `~/.margatroid`。
- `--config <PATH>` / `MARGATROID_CONFIG`：配置文件；默认读取数据目录下的
  `margatroid.toml`，默认文件不存在时允许启动，显式文件不存在时失败。
- `MARGATROID_LOG_TOKEN`：设置后启用带 bearer token 的日志 SSE 端点。

配置优先级固定为：CLI 参数 > 环境变量 > 配置文件 > 默认值。

```toml
[daemon]
bind = "127.0.0.1:3939"
data_dir = "./state"
log_stream_bearer_token = "replace-me"
```

配置中的相对 `data_dir` 以配置文件所在目录为基准。配置文件包含 bearer token 时，
Unix 权限不得允许 group/others 访问。数据目录固定为 `0700`，单实例 lock 文件为 `0600`。
