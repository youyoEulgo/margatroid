# LuaPlugin

LuaPlugin把`tool:<scope>/<name>:<tag>`资源注册为Agent工具，并使用内嵌Lua 5.4异步执行
`main.lua`。Lua工具被视为开发者主动安装的可信代码：完整Lua标准库和开放的文件、HTTP、JSON、日志
便利API均可使用，Plugin不构成安全沙箱。

工具包：

```text
tools/<scope>/<name>/<tag>/
├── tool.toml
├── input.schema.json
└── main.lua
```

`examples/tools/`保存可直接安装的完整示例：

```text
examples/tools/local/
├── list-directory/latest/
└── read-file/latest/
```

两个示例都把相对路径解析到`context.project_root`，同时保留绝对路径和父目录访问能力；它们既是
基础文件工具，也是编写Tool元信息、JSON Schema、跨平台路径处理和异步宿主调用的参考实现。

完整职责、接口和边界见`DESIGN.md`。
