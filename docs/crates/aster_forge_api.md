# aster_forge_api

`aster_forge_api` 提供框架无关的 API response helper，当前重点是分页、cursor、排序和 OpenAPI schema 条件派生。

它不依赖 Actix、Axum 或产品实体，handler 层只需要把请求参数映射成这些通用结构，再把结果包装回产品响应。

## 适用场景

- limit/offset 分页。
- cursor 分页。
- 常见 cursor 参数解析。
- 列表响应结构。
- `SortOrder` 的稳定序列化。
- debug + `openapi` feature 下的 `utoipa` schema 派生。

不适合放在这里的内容：

- 产品列表默认排序规则。
- 权限过滤。
- 数据库查询本身。
- API 错误状态码和本地化文案。

## Cargo feature

```toml
[dependencies]
aster_forge_api = { git = "https://github.com/AsterCommunity/AsterForge" }
```

OpenAPI 构建：

```toml
aster_forge_api = { git = "https://github.com/AsterCommunity/AsterForge", features = ["openapi"] }
```

`openapi` 只在 `debug_assertions` 下启用 `utoipa` 派生，避免 release binary 拉入文档生成负担。

## 分页参数

常用类型：

- `LimitOffsetQuery`
- `LimitQuery`
- `OffsetPage<T>`
- `CursorPage<T, C>`

典型接入：

```rust
let limit = query.limit();
let rows = repo::list(limit + 1).await?;
let page = aster_forge_api::CursorSlice::from_overfetched(rows, limit);
```

产品侧仍然负责：

- 查询时多取一条还是单独 count。
- cursor 字段对应哪个数据库索引。
- 是否允许客户端指定更大 limit。

## Cursor 解析

常用函数：

- `parse_id_cursor`
- `parse_string_id_cursor`
- `parse_datetime_id_cursor`
- `parse_datetime_string_cursor`
- `parse_sort_order_name_id_cursor`
- `parse_enabled_priority_id_cursor`

这些函数只做参数完整性校验。例如传了 `after_id` 却没传配套 timestamp，会返回 `ApiError`。产品侧应在 handler/service 边界把 `ApiError` 映射为自己的 bad request 错误。

## 排序

`SortOrder` 只表达 `Asc` / `Desc`，不表达产品字段名。字段白名单应该留在产品仓库，不要让客户端传任意列名后直接拼到数据库层。

## OpenAPI 接入

如果产品启用 OpenAPI：

```toml
[features]
openapi = ["aster_forge_api/openapi"]
```

然后把分页类型直接放进 route query 或 response schema。没有启用 feature 时，`ApiSchema` 是空 trait，不影响普通编译。

## 测试要求

- cursor 参数成对出现的错误路径。
- limit clamp 到产品允许范围。
- overfetch 后 `next_cursor` 是否正确。
- OpenAPI feature 下 schema 编译通过。

## 参考项目

- AsterDrive：文件、文件夹、分享、任务列表等 cursor 分页。
- AsterYggdrasil：管理员任务和用户列表等较轻 API。
