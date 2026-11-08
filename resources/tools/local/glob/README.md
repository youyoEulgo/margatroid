# Glob 文件查找

[Lua 入口](latest/main.lua) 使用通用宿主 API，不启动 shell 或外部 rg。
参数以 [schema](latest/input.schema.json) 为准，保持不变：
必填非空 `pattern`，可选非空 `path`，默认 `.`；拒绝额外字段。
相对路径基于项目根；参数来自 `margatroid.context.arguments`。

实现用 `margatroid.files:list` / `info` 在 Lua 中递归枚举。
`list` 无过滤地返回直接子项，含 name、绝对条目 path、自身 kind、size、可选 resolved_path 和 modified_at_ns。
符号链接自身类型与规范目标须区分，遍历行为严格保持现有 rg 语义。
文件路径交给 `margatroid.files:matches(path, pattern)`，它仅匹配路径字符串，不访问文件或递归。
匹配使用真正的 glob 语义，保留 slash、`**`、字符类、brace 与 anchoring；不得以 Lua pattern 代替。
无斜线模式在任意深度匹配 basename；含斜线模式保持 rg 路径匹配语义。
包括隐藏和被忽略文件；继续排除 `.git`、`.hg`、`.svn`、`.jj` 目录。
保留 `--no-config`，不从本机 rg 配置引入额外行为。

只返回文件，按现有修改时间降序排列，最多展示 100 个。
排序读取 info/list 的可选整数 modified_at_ns（Unix 纳秒）；缺失所需时间或元数据读取失败须明确报错，不能改为名称排序。
总数超过展示数时保留原有 omitted 数量和收窄搜索提示。
空结果返回 `No files found.`；路径中的 CR/LF 继续转义展示。
保持 30000 毫秒超时和现有 16 MiB 输出预算，超限明确失败。
保持无匹配、无效模式、路径/权限和遍历错误的现有区分与报错行为。
使用 `margatroid.task:return_result` 返回既有 `{ok,content}`，随后 `margatroid.task:finish`。
文件查找不依赖 shell-client；预期失败返回简洁结果，未预期异常由宿主自动记录原始诊断并生成简洁工具错误。
