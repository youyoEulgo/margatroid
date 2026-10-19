# Protocol

Protocol定义JSON信封、请求、回执、状态、历史和流分片。只依赖标准库及serde等通用库，不依赖领域core、plugin、address或World。领域转换、寻址和请求关联由DtoPlugin完成。

## 网络消息

入站信封为`{type,id,message}`，id非空且最多512个UTF-8字节，字段使用snake_case。实体地址与文件资源定位符作为字符串传输，不使用内部Entity、组件或句柄。

请求包含连接注册、Workspace列表与启停重启、Driver清单、目录浏览、状态查询、历史、MCL和回合中止；响应携带原网络请求ID。Workspace列表由DTO同步投影启动时已加载的内存配置，不触发磁盘扫描或领域查询事件。配置修改通过Margatroid统一重载生效，不提供Workspace独立配置变更和刷新接口。

## 状态与消息值

state.sync包含workspace_definitions、workspaces、agents、histories和drivers。agent_instance是拒绝迟到消息的不透明输出标识，不用于入站寻址。

网络消息独立定义System、Inject、User、Assistant、ToolCall结果和Error。图片放入独立images数组，元素为media_type与标准base64 data；支持PNG/JPEG/GIF/WebP，每条最多8张、每张解码后5MiB、合计20MiB。replay作为可选不透明JSON传输。

历史保留正文、元数据、顺序和created_at_ms；时间0表示未知。流分片关联请求ID、Agent、实例及sequence；finished表示显示流终态，不表示领域操作或清理完成。

## 校验

协议只校验JSON、字段形状及预算。总消息上限32MiB；递归消息深度最多32、节点最多4096，非图片消息值最多1MiB。

ProtocolErrorDto包含kind和message；message最多512个UTF-8字节，不包含凭据、图片或请求正文。注册身份、目标归属、业务状态及在途去重不属于协议校验。
