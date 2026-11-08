# 写入文本文件

[Lua 入口](latest/main.lua) 使用通用文件系统 API 写入并按需创建父目录，不启动 shell 或 mkdir。
参数以 [schema](latest/input.schema.json) 为准，保持不变：
必填非空 `path` 与字符串 `content`，可选布尔 `create_parent_directories`，默认 false。
拒绝额外字段；允许空 content，相对路径基于 Agent 项目根。
参数来自 `margatroid.context.arguments`，项目元数据来自 `margatroid.context`。

入口使用 `margatroid.files:write(path, bytes)` 完整替换一个文件。
files.write 不隐式创建父目录；文件工具不依赖 shell-client。
先校验 content 不超过 16 MiB 且是有效 UTF-8，再执行文件操作。
按实际 UTF-8 字节写入，保留内容与换行，不追加文本。
当且仅当 create_parent_directories 为 true，Lua 求父路径后显式调用：
`margatroid.files:create_directory(parent, true)`。
父路径为空等现有无需创建的情况继续跳过。
随后调用 files.write；创建目录失败时不继续写入。

保持现有覆盖语义与操作顺序，不额外引入备份或事务承诺。
create_parent_directories 为 false 时，父目录缺失必须明确失败。
保留路径、权限、目标类型、写入及完成写入错误的现有失败边界。
成功 content 仍是 JSON 文本 `{path,bytes,replaced:true}`。
path 是工具解析后的绝对路径，bytes 是 content 的实际字节数。
JSON 使用 `margatroid.data:encode`；保持 30000 毫秒超时。
使用 `margatroid.task:return_result` 返回既有 `{ok,content}`，随后 `margatroid.task:finish`。
预期文件错误返回简洁失败；未预期异常由宿主自动记录原始诊断并生成简洁工具错误。
