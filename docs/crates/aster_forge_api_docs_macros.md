# aster_forge_api_docs_macros

`aster_forge_api_docs_macros` 提供 `#[path(...)]` attribute macro，用来统一 OpenAPI route annotation 的写法。

它的设计目标是：业务代码保留一份 annotation，只有在 debug + `openapi` feature 下展开为 `utoipa::path`，普通 release 构建保持原函数不变。

## 适用场景

- 产品仓库想保留 OpenAPI metadata，但不想让 release 构建强依赖 OpenAPI 生成。
- route annotation 已经很多，不想用 `cfg_attr` 把 handler 写得很乱。

不适合放在这里的内容：

- OpenAPI document 组装。
- schema 注册列表。
- 产品 API tag 和安全 scheme 设计。

## Cargo feature

```toml
[dependencies]
aster_forge_api_docs_macros = { git = "https://github.com/AsterCommunity/AsterForge" }
```

启用 OpenAPI：

```toml
aster_forge_api_docs_macros = { git = "https://github.com/AsterCommunity/AsterForge", features = ["openapi"] }
```

## 接入方式

```rust
#[aster_forge_api_docs_macros::path(
    get,
    path = "/api/items",
    responses((status = 200, description = "List items"))
)]
async fn list_items() -> impl Responder {
    // handler
}
```

启用条件：

- `feature = "openapi"`
- `debug_assertions`

两个条件都满足时，宏展开为 `#[utoipa::path(...)]`。否则宏只返回原 item。

## 产品侧职责

产品仓库仍然需要自己维护：

- `OpenApi` derive 的 paths/schema 列表。
- API tag 命名。
- security scheme。
- 文档生成测试。

Forge 不知道产品路由结构，也不应该知道。

## 测试要求

- 普通构建下 handler 能编译。
- `--features openapi` 下 OpenAPI 生成测试能编译。
- route annotation 改动时至少有生成文档或 schema 快照测试。

## 参考项目

- AsterYggdrasil：可参考 OpenAPI debug feature 下 route annotation 的写法。
- AsterDrive：API 面更大，适合参考如何组织 OpenAPI paths 和 schema 列表。
