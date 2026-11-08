# Skill 注册工具

[Lua 入口](latest/main.lua) 通过 `margatroid.files`、`margatroid.data`、`margatroid.tools` 和 `margatroid.task` 完成注册与读取。Skill 元数据校验、清单收集和读取逻辑均在 Lua 中；独立 Skill 客户端 crate 与资源 bin 已删除，执行流程不启动子进程或交换 JSONL。

`skill_register` 仍只接受一个参数 `path`：本机标准 `SKILL.md` 文件路径，相对路径基于 Agent 项目根目录。例如 `{"path":"resources/skills/local/example-skill/SKILL.md"}`。标准校验依据 [Agent Skills specification](https://agentskills.io/specification)，使用与集成指导见 [Integrate skills](https://agentskills.io/integrate-skills)。

## 通用能力与 Lua 分工

入口从 `margatroid.context.arguments` 读取参数，其他元数据取自 `margatroid.context`；根对象不再提供 arguments 或 version。Skill 使用的通用接口为：

```text
margatroid.data:encode / decode（JSON、YAML 等通用格式）
margatroid.data.null
margatroid.files:read(path) -> bytes
margatroid.files:list(path) -> 直接子项，每项含 name 及 info 字段
margatroid.files:info(path) -> {path, resolved_path?, kind, size, modified_at_ns?}
```

`info.path` 是被查询条目的绝对路径，`kind` 是条目自身类型；`resolved_path` 在可取得时为规范目标路径。符号链接须明确查询其规范目标的 info，不能用链接自身的 kind/size 代替目标文件校验；无法取得所需规范路径时明确失败。`list` 只枚举直接子项，递归由 Lua 完成。

这些是通用格式编解码与文件方法，在当前工具沙箱执行；不新增 Skill 专属宿主 API。frontmatter 分割、标准字段校验、目录递归收集、schema 枚举与路径 binding 生成均留在 Lua。Skill 只使用所需方法，文件写入不属于注册或读取流程。

## 注册与读取

同一 Lua 入口按 `margatroid.context.binding` 分流。无 binding 时：解析输入路径 → `margatroid.files:read` → 分割独立 `---` 行包围的 YAML frontmatter → `margatroid.data:decode`（YAML） → 校验标准元数据 → 收集目录 → `margatroid.tools:register` 注入静态读取工具。以 `name` 为 alias、`description` 为描述，保存 `{kind:"skill", skill_md, files}` binding；读取 schema 只含可选参数 `path`，其枚举来自完整登记文件清单，默认 `SKILL.md`。

注入定义省略 `owner_task_id`，保持静态、无任务所有者；程序path由宿主固定为当前工具资源。注册短任务返回 alias、规范 Skill 路径及相对文件清单，然后调用 `margatroid.task:finish` 结束；不返回正文。静态入口继续随 Agent.Tools 存在，alias 冲突由宿主原子拒绝，不覆盖已有工具。

有 binding 时：校验登记成员与规范路径 → `margatroid.files:read` → 返回内容 → finish；不启动 bin、不交换 JSONL。省略 `path` 或传 `SKILL.md` 读取完整指令，传 `{"path":"references/details.md"}` 读取已登记资料。文本返回完整 UTF-8 内容；二进制仅返回实际路径、相对路径、类型和字节大小。脚本作为资料读取，不执行；指令及 `allowed-tools` 不改变宿主权限或工具选择。注册与读取都通过 `margatroid.task:return_result` 返回一次，再由 `margatroid.task:finish` 结束短任务。

注册与读取中的预期校验失败使用 `{skill_failure=true,message=...}` table 交给入口处理，返回 `Skill: ...` 失败内容，不携带 Lua 来源行号或 traceback，也不自动记异常日志。其他 Lua 异常通过 `error(result, 0)` 原样重新抛给宿主，由宿主记录原始诊断并生成简洁工具错误；Lua 不把 `tostring(error)` 拼进失败结果。

## 标准校验与文件边界

frontmatter 必须是映射，支持标准 YAML 语法，未知字段报错：

- `name`：1–64 个 ASCII 小写字母、数字或连字符；不得以连字符起止、包含连续连字符，且须等于规范 Skill 根目录名。
- `description`：非空字符串，最多 1024 个 Unicode 字符。
- `license`：可选字符串；`compatibility`：可选非空字符串，最多 500 个 Unicode 字符。
- `metadata`：可选字符串到字符串的映射，不隐式转换其他类型；`allowed-tools`：可选字符串，仅作为元数据与指令内容。

根目录为规范 `SKILL.md` 的父目录。Lua 使用 `margatroid.files:list` 与 `margatroid.files:info` 递归收集所有普通文件，包括 scripts、references、assets 和隐藏文件，不隐式忽略。仅接受根目录内的符号链接；越界或失效链接、目录链接循环和特殊设备文件使注册失败。

读取前重新检查登记成员、根目录与目标规范路径，按路径组件确认目标包含于规范根目录，再在当前工具沙箱内调用 `margatroid.files:read`。文件删除、越界重定向或根目录变化须明确失败。保留严格路径规范与包含性检查目标；此方案不承诺查询规范路径与读取之间的原子性，也不依赖尚未定义的 `openat` 或 capability 接口族。

新文件须重新登记才进入 schema 枚举，已有文件每次重新读取；不增加刷新、覆盖或卸载接口。UTF-8 有效且不含 NUL 的内容作为文本完整返回，其他内容作为二进制路径信息返回，空文件返回空文本。每个文件的读取预算为 8 MiB：先用 `margatroid.files:info(path).size` 检查再调用 `margatroid.files:read`，读取后再次检查实际字节数；超过预算的所有文件（含二进制）均明确报错，不读取大文件来判断类型，不静默截断。递归清单最多访问 10000 个文件系统条目，最终按相对路径排序，每条仅含 `path`、`kind:"file"` 与 `size_bytes`；包含空文件，且至少含 `SKILL.md`。入口与参数定义见 [工具清单](latest/tool.toml) 和 [注册参数 schema](latest/input.schema.json)。
