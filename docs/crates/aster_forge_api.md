# aster_forge_api

`aster_forge_api` 提供框架无关的强类型 REST envelope、分页、cursor、PATCH 三态字段、排序和 OpenAPI schema 条件派生。

它不依赖 Actix、Axum 或产品实体，handler 层只需要把请求参数映射成这些通用结构，再把结果包装回产品响应。

## 适用场景

- limit/offset 分页。
- `{code,msg,data?,error?}` 响应容器及可选强类型诊断。
- cursor 分页。
- 常见 cursor 参数解析。
- PATCH 请求三态字段。
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

Forge 的 OpenAPI 类型使用 `utoipa` 6。产品侧也应使用同一主版本；如果提供 Swagger UI，使用 `utoipa-swagger-ui` 10，避免同时引入 utoipa 5 和 6 导致 schema trait 或文档类型不匹配。生成文档默认仍为 OpenAPI 3.1.0，升级依赖不会自动切换到 3.2.0。

## REST Envelope

模块：`aster_forge_api::response`。

| 类型 | 责任 |
| --- | --- |
| `ApiResponse<T, C, D = ()>` | 数据、产品 code、产品 diagnostic 三个类型参数；字段私有，受控构造与反序列化校验状态 |
| `ApiResponseCode` | 产品实现 `is_success()`；Forge 不定义成功码或业务错误码 |
| `ApiErrorInfo<D>` | `retryable` 和可选的 typed diagnostic；产品决定是否可重试、是否暴露诊断及脱敏 |
| `ApiEmptyData` | 序列化为空对象，区分无数据和 `null` |
| `ResponseEnvelopeError` | 只描述 code/state 矛盾，产品决定如何处理自己的分类错误 |

最小产品接入：

```rust
use aster_forge_api::response::{ApiResponse, ApiResponseCode, ApiEmptyData};

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ProductCode { Success, ValidationFailed }
impl ApiResponseCode for ProductCode {
    fn is_success(&self) -> bool { matches!(self, Self::Success) }
}

let success = ApiResponse::<_, ProductCode>::ok(ProductCode::Success, vec![1, 2])?;
let omitted = ApiResponse::<(), ProductCode>::ok_empty(ProductCode::Success)?;
let empty_object = ApiResponse::<ApiEmptyData, ProductCode>::ok_empty_data(ProductCode::Success)?;
let failure = ApiResponse::<(), ProductCode>::error(ProductCode::ValidationFailed, "product message")?;
# Ok::<(), aster_forge_api::response::ResponseEnvelopeError>(())
```

| 构造/输入 | Wire 行为 |
| --- | --- |
| `ok(code, data)` / `ok_with_message(code, msg, data)` | 带 `data`、无 `error`；普通 `ok` 的 `msg` 为空字符串 |
| `ok_empty(code)` | 省略 `data` 和 `error` |
| `ok_empty_data(code)` | `data: {}`，省略 `error` |
| `ok(code, ())` / `ok(code, None::<DTO>)` | 明确输出 `data: null`；round-trip 保留这个字段 |
| `error(code, msg)` | 省略 `data`，附带 `error: {retryable:false}` |
| `error_with_details(code, msg, None)` | 同时省略 `data` 与 `error` |
| `error_with_details(..., Some(ApiErrorInfo { ... }))` | 使用产品提供的 retryable 与诊断，缺失 diagnostic 省略该字段 |

成功构造拒绝失败 code，失败构造拒绝成功 code。反序列化也拒绝成功带 `error`、失败带 `data`（包括显式 `null`）；unknown code 由产品的 typed enum 解析。缺失、null 与具体值分别处理，不把空对象与无数据合并。读取使用 `code()`、`message()`、`data()`、`error_info()`，不能通过公开字段绕过状态约束。

diagnostic 泛型只承载产品定义的结构。AD 的 `kind/message/field/scope`、AG 的诊断可见性和错误脱敏都留在产品；共享容器不判断可重试性，不自动重试，也不生成诊断消息。普通 `error()` 使用保守的不可重试默认，产品需要其他策略时显式调用 `error_with_details()`。

`ApiResponse` 实现 Serde，不实现 Actix `Responder` / Axum `IntoResponse`。产品 handler 在自己选定的状态与 headers 上附加 JSON；业务权限、认证、cookie、Retry-After、i18n 和协议响应不由 Forge 接管。OAuth/OIDC、SAML、WebDAV 等协议端点不强制使用 REST envelope。产品把 `OffsetPage` / `CursorPage` 直接作为数据类型，不新增分页模型。

## Envelope OpenAPI

默认构建无需 web framework，debug + `openapi` 构建提供泛型 schema。单位值字段被描述为可省略的 `null`，不会把无数据响应的 `data` 或无诊断响应的 `diagnostic` 生成成必填 `unknown`。嵌套 DTO 的 schema 会一并收集。

Utoipa 注册和 route annotation 使用**完整泛型类型**；仅写产品 `type Response = ApiResponse<...>` 的 alias 会丢失宏可见的泛型参数，导致组件名称碰撞。可以使用产品命名空间的 `#[schema(as = ...)]` 区分不同 DTO/code 的同名结构。

```rust
use aster_forge_api::response::{ApiResponse, ApiResponseCode};
use utoipa::OpenApi;

#[derive(serde::Serialize, utoipa::ToSchema)]
enum ProductCode { Success, InvalidInput }
impl ApiResponseCode for ProductCode {
    fn is_success(&self) -> bool { matches!(self, Self::Success) }
}
#[derive(serde::Serialize, utoipa::ToSchema)]
struct ProfileDto { id: u64 }
type NoData = ();

#[derive(OpenApi)]
#[openapi(components(schemas(
    ApiResponse<ProfileDto, ProductCode, NoData>,
    ApiResponse<NoData, ProductCode, NoData>
)))]
struct ApiDoc;
let document = ApiDoc::openapi();
```

生成组件包含 `ApiResponse_ProfileDto_ProductCode_TupleUnit` 等具体名称。产品若需要一个稳定的 schema 名，可保留 `#[serde(transparent)]` 的产品 adapter（如模板的 `ErrorResponse`），但应让它承担实际产品错误映射、状态或 schema 边界，而不是复制 envelope 字段和序列化规则。

`templates/aster-service-actix` 的 API 404 已使用共享 envelope，并保持 HTTP 404；健康探针仍是原来的 `StatusResponse`。模板同步生成 OpenAPI 与 TypeScript SDK。AD/AG 的真实迁移由 [AD #633](https://github.com/AsterCommunity/AsterDrive/issues/633) 和 [AG #14](https://github.com/AsterCommunity/AsterGate/issues/14) 跟踪，模板验收不代表产品迁移已完成。

## 源码结构

`lib.rs` 只声明模块和公共导出；`error.rs`、`cursor.rs`、`pagination.rs`、`patch.rs`、`schema.rs`、`sort.rs`、`response.rs` 各自持有对应机制与测试，envelope 的 OpenAPI 适配位于 `response/schema.rs`。已有 crate 根路径 import 保持有效。

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

## PATCH 三态字段

类型：

- `NullablePatch<T>`
- `deserialize_nullable_patch_option`

`NullablePatch<T>` 用于区分 PATCH DTO 中的三种状态：

- `Absent`：字段未传，保持原值。
- `Null`：字段显式传入 `null`，清空原值。
- `Value(T)`：字段传入具体值，更新为新值。

常见写法：

```rust
#[derive(serde::Deserialize)]
struct UpdateItemRequest {
    #[serde(default)]
    title: aster_forge_api::NullablePatch<String>,
    #[serde(
        default,
        deserialize_with = "aster_forge_api::deserialize_nullable_patch_option"
    )]
    description: Option<aster_forge_api::NullablePatch<String>>,
}
```

如果字段本身不是 `Option`，直接用 `#[serde(default)]` 即可。字段本身需要 `Option<NullablePatch<T>>` 时，使用 `deserialize_nullable_patch_option()` 保留显式 `null`。

产品 service 应该在更新逻辑里显式匹配三态，不要把 `Null` 和 `Absent` 混掉。

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
- PATCH DTO 中 omitted/null/value 三态。
- overfetch 后 `next_cursor` 是否正确。
- OpenAPI feature 下 schema 编译通过。
- Envelope 成功/失败 code 分类、字段省略、显式 null、空对象、空集合、Unicode、整数边界、typed diagnostic、反序列化矛盾和 duplicate field。
- 至少两个独立产品 code、多个 DTO 和嵌套 DTO 的 OpenAPI 名称与引用；用真实生成的 SDK 做成功/错误反例类型检查。
- 产品接入使用真实 Actix/Axum 请求验证 HTTP 状态、headers、错误映射、前端已知/未知 code 和非 JSON HTTP 失败。

```bash
cargo test -p aster_forge_api
cargo test -p aster_forge_api --features openapi
cargo clippy -p aster_forge_api --all-targets --all-features -- -D warnings
bash scripts/test-api-response-sdk.sh
```

SDK 验证脚本从 lib schema 测试导出真实 OpenAPI，使用固定的 `openapi-typescript` 与 TypeScript 6 生成并严格检查代码、DTO、嵌套引用、字段省略和错误反例。临时产物路径会显示在输出中。

## 参考项目

- AsterDrive：文件、文件夹、分享、任务列表等 cursor 分页。
- AsterYggdrasil：管理员任务和用户列表等较轻 API。
