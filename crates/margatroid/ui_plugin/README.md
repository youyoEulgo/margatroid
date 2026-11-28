# ui_plugin

`ui_plugin` 将构建好的 Web UI 静态文件嵌入后端二进制，并通过原生 Axum 路由提供页面。它适合由同一个 daemon 分发页面和后端接口的部署方式。

## 接入静态页面

应用完成 `AxumPlugin` 装配后，在首次 tick 前调用 `app.add_plugin(UiPlugin::new())`。插件将页面 Router 合并到 HTTP 服务。服务装配顺序见 [axum_plugin 接入示例](../axum_plugin/README.md)。

也可以将页面 Router 合并进自行管理的原生 Axum Router：

```rust
use axum::Router;
use ui_plugin::UiPlugin;

fn main() {
    let router: Router = Router::new().merge(UiPlugin::router());
    let _application_routes = router;
}
```

`UiPlugin` 提供插件安装与 `router()` 两种入口；`UiPluginInstalled` 是 World 中的安装标记资源。通过插件入口重复安装会触发安装检查 panic。

## 路由与资源生命周期

- `GET /` 返回嵌入的 `index.html`。
- `GET /assets/{*path}` 根据完整请求路径读取嵌入资源。
- fallback 根据请求路径查询其余嵌入文件。

资源通过 `rust-embed` 从 [Web UI 构建目录](../../../ui/dist/) 收集。发布构建将资源嵌入二进制；更新发布页面时，先生成 Web UI 构建产物，再重新编译 daemon。

响应依据文件扩展名设置 Content-Type。资源查找失败返回 `404 Not Found`，响应构造失败返回 `500 Internal Server Error`。浏览器的 WebSocket 协议入口由 [dto](../dto/README.md) 装配，页面服务与协议路由共同使用已有 HTTP 服务。

入口、资源路径、MIME 映射与响应行为见[后端实现](src/lib.rs)，结构与函数说明见 [DESIGN.md](DESIGN.md)。
