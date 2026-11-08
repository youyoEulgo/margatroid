# Grep 内容搜索

[Lua 入口](latest/main.lua) 使用通用宿主 API，不启动 shell 或外部 rg。
参数以 [schema](latest/input.schema.json) 为准，保持不变：
必填非空 `pattern`，可选非空 `path`（默认 `.`）及 `include`；拒绝额外字段。
`path` 可指文件或目录，相对路径基于项目根；参数来自 `margatroid.context.arguments`。
`include` 只接受一个正向 glob，以 `!` 开头仍明确失败。

实现通过 `margatroid.files:list` / `info` 遍历、`margatroid.files:read` 读取原始 bytes。
用 `margatroid.files:matches(path, pattern)` 处理 include，用 `margatroid.text:search(pattern, content)` 搜索每行。
search 使用 Rust regex/ripgrep 默认正则语义，返回 `{start,end}` 匹配数组；偏移是 UTF-8 字节半开区间，零宽匹配有限前进。必须保留 Unicode、大小写默认值及内联 flags。
不得使用 Lua pattern 假装正则，也不得把正则改为字面量搜索。
继续保持 `--json --no-config --color never` 对应的搜索和展示语义。
逐行匹配、行号、换行处理、文件/目录输入、隐藏/忽略文件、二进制、编码及符号链接处理均严格保持现有 rg 行为。
Lua 负责目录递归、隐藏文件及 `.gitignore` / `.ignore` / `.rgignore` 和 include 选择，可使用资源共用脚本；不得引入 Glob 的 `--hidden --no-ignore` 行为。

最多展示 250 个匹配事件，每项为 `path:line_number:preview`。
保留现有结果顺序与计数，不擅自加入排序；多匹配同一行的计数保持现状。
预览去除尾部 CR/LF，超过 2000 字节时按 UTF-8 边界截断并保留提示。
非文本路径与内容保持原有 base64 展示；编解码使用 `margatroid.data`。
路径中的 CR/LF 转义，超出展示数保留 omitted 数量和收窄搜索提示。
空结果返回 `No matches found.`；保持 30000 毫秒超时及 16 MiB 输出预算。
无匹配与无效正则、路径/权限错误、输出超限须保持现有区分，错误不能静默跳过。
使用 `margatroid.task:return_result` 返回既有 `{ok,content}`，随后 `margatroid.task:finish`。
实现不依赖 shell-client；预期失败返回简洁结果，未预期异常由宿主自动记录原始诊断并生成简洁工具错误。
