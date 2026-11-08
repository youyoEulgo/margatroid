# 列出目录

[Lua 入口](latest/main.lua) 直接使用通用文件系统 API，不启动 shell 或 find。
参数以 [schema](latest/input.schema.json) 为准，保持不变：
只接受可选非空 `path`，默认 `.`，拒绝额外字段。
相对路径基于 Agent 项目根，绝对路径直接使用。
参数来自 `margatroid.context.arguments`，项目元数据来自 `margatroid.context`。

入口直接调用 `margatroid.files:list(path)`，文件工具不再依赖 shell-client。
通用 list 无过滤地返回直接子项，不递归；包含隐藏条目。
每项含 name 及 info 字段：path、可选 resolved_path、kind、size、可选 modified_at_ns（Unix 纳秒整数）。
path 是查询条目的绝对路径，kind 表示条目自身类型。
符号链接保持 symlink 类型，不用规范目标类型覆盖自身类型。
resolved_path 在可取得时表示规范目标；不得因此展开目标目录。

工具对外结果保持现有数组投影，每项仅含 name、path、kind。
kind 保持 file、directory、symlink、other 的现有映射。
Lua 按 name 升序排序，不依赖底层 list 的返回顺序。
JSON 使用 `margatroid.data:encode`，空目录仍返回文本 `[]`。
保留完整名称与路径，包括含制表符、换行等特殊字符的条目。
保持 30000 毫秒超时及现有 16 MiB 输出预算，超限明确失败。
不存在、非目录、权限和读取失败保持现有错误边界，不能当作空目录。
使用 `margatroid.task:return_result` 返回既有 `{ok,content}`，随后 `margatroid.task:finish`。
预期文件错误返回简洁失败；未预期异常由宿主自动记录原始诊断并生成简洁工具错误。
