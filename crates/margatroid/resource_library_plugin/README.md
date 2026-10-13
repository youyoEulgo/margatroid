# 已合并的基础设施

此目录仅保留历史设计归档，不再是可编译 crate。

资源地址、BTreeMap 资源集合、类型注册及受检 World 查询已统一至 [resource_id_plugin](../resource_id_plugin/DESIGN.md)。领域资源条目定义于共享 types，由五类加载插件写入目标实体的集合。不再创建共享资源宿主实体、维护 ResourceOwners 或执行引用计数回收。
