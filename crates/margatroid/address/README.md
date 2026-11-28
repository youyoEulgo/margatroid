# address

`address` 把对外地址转换为 ECS 实体或资源文件路径。开发者在接收用户输入、查找运行节点、导入文件资源和列举资源时使用这个 crate。

## 两种地址

| 对象 | 示例 | Rust 类型 |
| --- | --- | --- |
| 运行节点 | `/demo`、`/demo/assistant` | `Address` |
| 文件资源 | `tool:margatroid/task-list:dev` | `ParsedAddress` |

`Address` 是挂载到实体上的组件。`ParsedAddress` 保存资源的类型、作用域、名称和标签。两类地址分别进入实体查找和文件解析流程。

## 查找实体

先校验地址，再将组件写入 World，随后通过 `WorldAddressExt` 查找对应实体：

```rust
use address::{Address, WorldAddressExt};
use core_plugin::World;

fn main() -> Result<(), address::AddressError> {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert_component(entity, Address::new("/demo/assistant")?);

    assert!(world.address_exists("/demo/assistant")?);
    assert_eq!(world.entity_by_address("/demo/assistant")?, entity);
    Ok(())
}
```

调用方在写入地址前完成查重。`entity_by_address` 返回完整的 `Entity`，后续领域操作以该实体为关联。查找结果分别通过 `EntityMissing` 和 `DuplicateAddress` 表示空结果与地址冲突。

## 解析文件资源

`ParsedAddress::parse` 解析完整的 `type:scope/name:tag`。`resolve_file` 接收对应类型的根目录和资源地址，返回规范化后的入口文件路径。

| 类型 | `ResourceRoots` 下的目录 | 入口文件 |
| --- | --- | --- |
| `driver` | `agent-drivers` | `base.lua` |
| `tool` | `tools` | `tool.toml` |
| `prompt` | `prompts` | `prompt.toml` |
| `sandbox` | `sandboxes` | `policy.json` |

例如，工具地址 `tool:margatroid/task-list:dev` 对应工具根目录下的 `margatroid/task-list/dev/tool.toml`。文件解析逐层检查真实路径与资源根的包含关系。

`query_resources` 接收 `ResourceRoots` 和 `ResourceQuery`，完成一次资源枚举。筛选项为类型、作用域和名称，结果包含全部匹配标签，并按地址各段排序。每条 `ResourceFile` 包含资源地址、入口路径及可选的工具启动清单路径。异步调用方通过 blocking worker 执行这类文件系统操作。

## 接口与示例

- [公开导出与World扩展](src/lib.rs)
- [节点地址组件](src/components.rs)
- [资源地址、根目录和查询类型](src/types.rs)
- [文件解析与枚举](src/handler.rs)
- [节点查找示例](tests/parsing.rs)、[文件解析示例](tests/files.rs)、[资源查询示例](tests/resources.rs)
- [实现设计](DESIGN.md)
