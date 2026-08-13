# WorkflowPlugin

WorkflowPlugin注册`tool:builtin/workflow-loader:latest`并接收Workflow定义检查和调用事件。
当前只验证完整Workflow资源目录并返回未实现的占位工具响应，不执行Workflow步骤。
