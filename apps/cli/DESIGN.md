# MargatroidCli

## 目录选择

catalog、注册启动和成员管理接线已恢复并完成对应验证；创建、重命名、删除及发现能力已接通。验证结论限于实际执行场景，不将 API 存在或历史测试等同于普遍可用性保证。

CLI 只负责已注册工作区发现、选择、启动/停止请求与日志显示；不再依赖 Compose，不读取或上传 YAML，不创建项目级配置、数据库或其它产物。

```text
Cli::parse
-> run(WorkspaceCommand::Up { name: Option<String>, backend_url })
-> WebSocket 连接
-> connection.register(client_type=cli)，等待匹配回执
-> workspace.catalog.query {id,message:{}}，等待匹配 workspace.catalog
-> catalog_candidates(entries, name, canonical_cwd)
-> choose_workspace(socket,candidates,stdin_is_terminal)
-> workspace.start {workspace: ResourceIdDto, expected_revision: String}
-> handle_workspace_start_reply + 日志/心跳/关闭信号
-> workspace.stop -> wait_for_stop_ack -> 关闭连接
```

### 选择

显式名称按 catalog.name 精确匹配，可从任意目录调用。无名称时规范化现有 CWD，只比较 catalog.definition.project_root 规范路径是否完全相等；不向祖先目录搜索，不读取配置正文。远程路径无法在本地规范化时不能匹配，应指定名称。

零匹配报错；一项直接选择；多项非 TTY 报错；多项 TTY 输出编号、名称、项目根与生命周期。`parse_choice` 接受有效编号，空输入或 q 返回取消，非法输入报错。只有成功选择一个有效定义才发送一次启动请求。

交互读取在独立的 detached thread 中等待，通过 oneshot 返回结果；异步选择循环继续处理 WebSocket Ping/Pong、日志和关闭信号，取消不会因阻塞 stdin 阻止进程结束。没有运行会话内的用户消息输入。

### 请求与权威

catalog 条目携带 id/name/path/revision/definition?/error?/lifecycle。CLI 不推断运行状态；无效定义报可操作错误。启动只提交条目 ID 和当前 revision，后端重新读取规范定义并处理并发修改；旧定义不会随客户端覆盖到服务端。

`wait_for_register_ack`、`wait_for_catalog`、`wait_for_stop_ack` 都匹配请求 ID、处理文本/二进制和心跳，具有 5 秒截止时间及关闭/断线错误。日志继续输出 RFC 3339 UTC、等级和目标，终端 ANSI/重定向纯文本。

### 生命周期

WorkspaceStarted 是工作区成功事实，member_failures 逐项显示但不终止会话。成功前匹配 WorkspaceStartFailed/RequestFailed 才作为启动失败；成功后重复或迟到失败不改变会话。

关闭信号 future 跨消息保留；启动中首次信号保留停止意图，启动成功后发送停止。等待停止期间的重复信号、超时、断线或失败回执报错。CLI 不自行清理后端工作区，也不因成员失败断开。

## 验证

恢复后须以 `cargo test -p margatroid_cli` 验证精确规范路径、符号链接、名称/未知名、非终端歧义、选择取消/非法编号、注册/停止心跳和二进制回执、启动拒绝、成员失败后持续连接、启动中 SIGTERM 与已启动工作区停止回执。

历史基线不提供当前通过数量；本次仅改文档，未运行测试或构建。
