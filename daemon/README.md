# margatroidd

`margatroidd` 是 Margatroid V3 daemon 的 composition root，只负责创建 ECS App、
安装默认 Plugin 组合并运行主循环。

```bash
cargo run -p margatroidd
```

环境变量：

- `MARGATROID_BIND`：监听地址，默认 `127.0.0.1:3000`。
- `MARGATROID_LOG_TOKEN`：设置后启用带 bearer token 的日志 SSE 端点。
