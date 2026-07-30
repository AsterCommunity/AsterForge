# aster_forge_webdav

`aster_forge_webdav` 是 Aster 产品共用的 WebDAV 协议边界。它负责把 HTTP/WebDAV 输入解析为强类型请求，校验路径、`Depth`、`Destination`、`If`、ETag 和日期条件，并定义协议响应、产品 backend port 与操作事件。

这个 crate 不拥有产品文件业务。认证账号、workspace scope、权限、ORM entity、存储策略、quota、版本落库和审计展示仍然留在产品仓库。

流式 Multi-Status 可通过 `DavCancellationToken` 和
`multistatus_stream_response_with_cancellation` 把 transport body drop 传播给产品 backend。
Forge 负责共享 cancellation 生命周期；产品负责选择执行时间上限，并在目录分页、属性、锁和
repository await 边界检查同一个 token。

## Cargo 接入

```toml
[dependencies]
aster_forge_webdav = { git = "https://github.com/AsterCommunity/AsterForge", features = ["actix"] }
```

默认 feature 只包含 transport-neutral 协议内核。Actix 产品启用 `actix`，使用 `aster_forge_webdav::actix` 完成请求和响应类型转换。

## 能力声明与响应快照

产品通过 `DavCapabilityProvider` 提供类型化能力声明，并由 associated profile 声明编译期能力上限。每个 RFC 扩展都有独立 support trait 和 marker；选择 marker 但遗漏对应 impl 会直接编译失败：

```rust
use aster_forge_webdav::{
    DavClass3Profile, DavCollectionSyncExtension, DavQuotaExtension,
    dav_capability_profile,
};

type ProductWebDavProfile = dav_capability_profile!(
    DavClass3Profile;
    DavQuotaExtension,
    DavCollectionSyncExtension,
);
```

DeltaV 不再压缩成一个 `versioning` 布尔值。`version-control`、`checkout-in-place`、
`version-history`、`workspace`、`update`、`label`、`working-resource`、`merge`、`baseline`、
`activity` 和 `version-controlled-collection` 是独立 package。marker 会把 RFC prerequisite
加入静态 profile；例如 `DavWorkspaceExtension` 同时要求并包含 version-control、
checkout-in-place 和 version-history。运行时 declaration 仍必须显式投影当前目标真正启用的完整
package 集，且不能超过静态上限。

typed catalog 还包含 WebDAV ACL（[RFC 3744](https://www.rfc-editor.org/rfc/rfc3744.html)）、
SEARCH（[RFC 5323](https://www.rfc-editor.org/rfc/rfc5323.html)）、Quota（[RFC 4331](https://www.rfc-editor.org/rfc/rfc4331.html)）、
Collection Sync（[RFC 6578](https://www.rfc-editor.org/rfc/rfc6578.html)）、Extended MKCOL（[RFC 5689](https://www.rfc-editor.org/rfc/rfc5689.html)）、
Current Principal（[RFC 5397](https://www.rfc-editor.org/rfc/rfc5397.html)）、Ordered Collections（[RFC 3648](https://www.rfc-editor.org/rfc/rfc3648.html)）、
Redirect References（[RFC 4437](https://www.rfc-editor.org/rfc/rfc4437.html)）、Bindings（[RFC 5842](https://www.rfc-editor.org/rfc/rfc5842.html)）、
POST Add Member（[RFC 5995](https://www.rfc-editor.org/rfc/rfc5995.html)）和 Prefer（[RFC 8144](https://www.rfc-editor.org/rfc/rfc8144.html)）。Forge 只为标准规定的 package 输出 DAV
token；SEARCH 使用 `DASL`，Quota、Sync、Current Principal、Add Member 和 Prefer 不发明 token。

根据 [RFC 4918 Section 18.3](https://www.rfc-editor.org/rfc/rfc4918.html#section-18.3)，
Class 3 表示新版基础语义，只要求 Class 1，**不蕴含 Class 2 锁**。因此使用
`DavClass3Profile` 得到 `DAV: 1, 3`；同时实现独立锁能力并选用
`DavClass2And3Profile` 才得到 `DAV: 1, 2, 3`。

provider 根据 `DavCapabilityTarget` 和 `DavCapabilityContext` 投影当前请求的资源、principal、
权限和运行时开关：

```rust
let snapshot = aster_forge_webdav::plan_capabilities_with_provider(
    &provider,
    &target,
    &context,
)
.await?;

let options = aster_forge_webdav::options_response(&snapshot);
let rejected = aster_forge_webdav::method_not_allowed_response(&snapshot);
```

Rust 没有运行时 trait 反射，Forge 通过显式的 `DavCapabilityProvider` 聚合产品 impl；Forge 不扫描 handler，也不从字符串推测能力。provider 可以把资源类型、mount root、principal、权限、锁配置和扩展状态合并成 `DavCapabilityDeclaration`。`plan_capabilities` 校验声明并生成不可变的 `DavCapabilitySnapshot`。

同一个 snapshot 是 `Allow`、`DAV`、`DASL`、`Accept-Patch`、OPTIONS、405、dispatch、body
policy、REPORT gate、live-property catalog 和 RFC 8144 preference 的唯一事实源。
`gate_method` 把已知和未知方法接入同一 405 gate，产品不直接拼协议 discovery header。
SEARCH grammar 使用 `DavSearchGrammar` 分别声明 DASL coded-URL 与 XML namespace/local-name，
不会错误地从 URI 猜 QName。

provider trait 使用原生 `async fn` 和泛型静态分发，不为 capability lookup 引入
`Pin<Box<dyn Future>>`。package、method 和 resource set 使用定宽 bitset；descriptor 和 grammar
使用静态 slice，不建立运行时字符串 registry。RFC 8144 的 `Preference-Applied` 从固定规范值中
选择，不为每个请求拼接 `String`；request plan 只给出 eligibility，response composer 必须把实际
执行的 preference set 传给 `preference_applied_header`，失败响应不会误报已应用的行为。

## Live property catalog 与 quota

Forge 维护标准 live property 的静态 catalog，并从 capability snapshot 投影
`supported-method-set`、`supported-live-property-set`、`supported-report-set` 和
`supported-query-grammar-set`。package descriptor 负责 discovery 与 dispatch gate；每个高级
REPORT 的具体请求 grammar 和执行 handler 仍由对应协议模块或产品 adapter 实现，声明 package
不等于伪装成所有操作已经落地。

PROPFIND 先通过 `live_property_requirements` 汇总本次真正需要的 metadata、lock、dead property、
quota、principal、sync token、add-member 和扩展值。`DavLivePropertyProvider` 每个资源只取得一次
batch snapshot，再由 `build_live_propfind_item_with_provider` 生成 propstat；不会按属性触发异步
N+1，也不会为 catalog 建立请求期集合。

RFC 4331 quota 语义固定为：

- `quota-used-bytes` 与 `quota-available-bytes` 默认不进入 `allprop`，显式 `include` 或 `prop` 才取值；
  ACL、DeltaV、Ordered Collections、Redirect References、Bindings、Sync 和 Add Member 中标准原文
  明确要求省略的扩展属性也遵循同一规则。
- `propname` 只列出当前有定义的 quota 属性；无限额度用 `available_bytes: None` 表示，此时省略
  `quota-available-bytes`，显式请求得到 404 propstat。
- collection/mount root 启用 quota 后必须提供 `quota-used-bytes` 快照；缺失返回 typed
  representation error，而不是猜零。file 上未定义的 quota 值按 404 property 处理。

产品仍拥有 quota 计算、权限、事务和持久化；Forge 只拥有 RFC 属性选择、发现、值形状和错误分类。

## PUT、partial PUT 与 PATCH

普通 PUT 只由 `DavMethod::Put` 控制，语义始终是完整替换。partial PUT、RFC 5789 PATCH 和私有 `X-Update-Range` 不共享开关，默认全部关闭：

- 未声明 partial PUT 时，任何 PUT `Content-Range` 都在正文和存储写入前规划为 `400 Bad Request`。
- 未声明私有兼容能力时，PUT `X-Update-Range` 同样规划为 `400`，不会被当作普通 PUT 忽略。
- 未声明具体 patch document media type 时，PATCH 不得出现在 method set，OPTIONS 也不会输出 `Accept-Patch`。

产品用静态 format slice 声明 PATCH，不需要运行时字符串注册表或 handler map：

```rust
use aster_forge_webdav::{
    DavMethod, DavMethodSet, DavPartialPutCapability, DavPatchBodyPolicy,
    DavPatchCapability, DavPatchFormat, DavWriteCapabilities, DavWritePrecondition,
};

static PATCH_FORMATS: [DavPatchFormat; 1] = [DavPatchFormat {
    media_type: "application/merge-patch+json",
    body_policy: DavPatchBodyPolicy::Bounded { maximum: 64 * 1024 },
    precondition: DavWritePrecondition::RequireStrongIfMatch,
}];

declaration.methods = DavMethodSet::from_methods(&[
    DavMethod::Options,
    DavMethod::Put,
    DavMethod::Patch,
]);
declaration.writes = DavWriteCapabilities {
    partial_put: DavPartialPutCapability::ContentRangeBytes {
        precondition: DavWritePrecondition::RequireStrongIfMatch,
    },
    patch: DavPatchCapability::Formats(&PATCH_FORMATS),
    ..DavWriteCapabilities::default()
};
```

provider 的 associated profile 必须同步声明静态上限。partial PUT 使用 `DavWithPartialPut<Base>`，PATCH 使用 `DavWithPatch<Base>`，私有 range-update 使用 `DavWithPrivateUpdateRange<Base>`；wrapper 可以组合。每个 wrapper 分别要求 provider 实现 `DavPartialPutSupport`、`DavPatchSupport` 或 `DavPrivateUpdateRangeSupport`，缺少 impl 会在编译期报错。运行时 declaration 仍按资源类型、权限和配置投影，但不能超过静态 profile。

PUT handler 把同一个 capability snapshot 交给 `plan_put_request`，再按 `DavPutWritePlan::Replace`、`Partial` 或 `PrivateUpdateRange` 调用产品 adapter。Forge 使用 `headers::ContentRange` 解析 RFC 9110 `bytes` range，拒绝 unsatisfied、重复、溢出、非法 complete length 和已声明 `Content-Length` 不一致；partial plan 返回 offset、payload length 与可选 complete length。stream 的实际字节数仍由产品 adapter 在提交前核验。

PATCH handler 使用 `plan_patch_request` 按结构化 `Content-Type` 选择 `DavPatchFormat`。缺失、畸形、重复或未声明 media type 返回 `415 Unsupported Media Type`，并携带 snapshot 生成的 `Accept-Patch`。plan 返回 format 和 `DavBodyPolicy`；Actix adapter 只按这个已规划 policy 收集 bounded body，stream policy 保留 payload 流：

```rust
let plan = aster_forge_webdav::plan_patch_request(&snapshot, &headers, resource)?;
let prepared = aster_forge_webdav::actix::prepare_request_body(
    plan.body_policy,
    &mut payload,
)
.await?;
```

以上能力只定义协议和 transport contract。产品 adapter 负责 partial write 的 staging、PATCH 的完整原子应用、provider session、quota、checksum、dedup/refcount、失败清理、audit 和最终 commit。依据 [RFC 9110 Section 14.5](https://www.rfc-editor.org/rfc/rfc9110.html#section-14.5)，partial PUT 只在产品确认的 client private agreement 下启用；PATCH 行为与 `Accept-Patch` 以 [RFC 5789](https://www.rfc-editor.org/rfc/rfc5789.html) 为准。

## 下载与写入 ports

读取、顺序写和随机写是三组独立能力：

- `DavDownloadSource` 提供下载 metadata、`open_full` 和 `open_range`。每次打开返回 `DavOpenedDownload`，同时携带 stream 与精确 `expected_length`。
- `DavWriteSystem` 打开 `DavWriteHandle`。普通 writer 只接收顺序 `Bytes` chunk，并通过 `finish` 提交或通过 `abort` 清理；它不包含 read、seek 或虚假的随机写能力。
- `DavRandomWriteSystem` 是 partial-write adapter 才实现的独立 port。`DavRandomWriteHandle::write_at` 显式接收 offset，不把远端对象存储伪装成本地 seekable file。

这三组新 port 使用 associated metadata/handle 和 `impl Future` 静态分发。Forge 不要求为 open、每个写入 chunk、finish 或 abort 分配 boxed future/handle；产品只有在自身需要运行时异构 backend 时才在自己的 adapter 边界做 type erasure。下载 body 最终保留一次现有 stream type erasure，用于把异构 storage stream 放入 transport-neutral response。

GET/HEAD handler 先用 metadata 调用 `plan_download_response`，再把选出的 `DavDownloadBody` 交给 `open_download`。Forge 会选择 full/range open，并核对 backend 声明的长度与协议 plan；HEAD、304、412 和 416 的 empty body 不会打开 storage stream。

```rust
let plan = aster_forge_webdav::plan_download_response(
    &headers,
    false,
    metadata.len(),
    content_type,
    metadata.etag().as_deref(),
    metadata.modified()?,
)?;

let opened = aster_forge_webdav::open_download(
    &downloads,
    &path,
    plan.body,
)
.await?;
```

普通 `plan_download_response` 保持 single-range 高效路径；需要 multipart 时，产品必须显式传入
`DavMultiRangePolicy`：

```rust
let limits = aster_forge_webdav::DavMultiRangeLimits::new(
    8 * 1024, // Range header bytes
    16,      // raw range specs
    8,       // final segments
    64 * 1024 * 1024, // selected representation bytes
    8,       // backend range opens
);
let plan = aster_forge_webdav::plan_download_response_with_multi_range(
    &headers,
    false,
    metadata.len(),
    content_type,
    metadata.etag().as_deref(),
    metadata.modified()?,
    aster_forge_webdav::DavMultiRangePolicy::new(
        limits,
        80, // coalesce gaps no larger than the multipart overhead budget
        aster_forge_webdav::DavRangeLimitBehavior::IgnoreRange,
    ),
)?;
```

多段规划遵循 [RFC 9110 Section 14.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-14.2)、
[Section 14.6](https://www.rfc-editor.org/rfc/rfc9110.html#section-14.6) 和
[Section 15.3.7.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-15.3.7.2)：单个 raw
range 永远走普通 `206`，多个 raw range 才允许生成 `multipart/byteranges`；顶层响应不带
`Content-Range`，每个 part 都带自己的 `Content-Type` 与 `Content-Range`。不可满足的成员会被
丢弃，全部不可满足时返回 `416`；重叠、相邻和不超过显式阈值的间隔按请求顺序合并。

所有 hard limits 在 backend 打开前生效。multipart `open_download` 会先为每个最终 segment
调用一次 `DavDownloadSource::open_range`，核对每个 stream 的精确长度，再返回一个增量 framing
stream；它不会把 representation 拼成完整 `Vec<u8>`。short read、过读或中途 backend error
会直接结束响应流，不补零，也不会伪造 closing boundary。客户端取消时，尚未消费的 backend
streams 会随 transport stream 一起释放。

PUT 的 `DavPutWritePlan::Replace` 使用 `DavWriteSystem`；`Partial` 只在声明能力后使用 `DavRandomWriteSystem` 或产品自己的原子 staging/session adapter。LOCK 创建空 lock-null resource 时，`ensure_lock_target_exists` 同时接收资源 backend 和顺序 write backend，并以 `finish` 作为创建提交点。

这一版直接移除了混合 `DavFile` 与 `OpenOptions`。下游迁移时把原 `open` 拆为 download、sequential write 和可选 random write impl；不保留只返回 unsupported/forbidden 的旧 read/seek facade。

## 协议所有权

Forge 负责：

- `DavPath` 的百分号解码、dot-segment 规范化和 mount escape 拒绝。
- WebDAV 方法、`Depth`、`Overwrite`、`Destination`、`If`、`Timeout` 和 `Lock-Token` header 解析。
- `If` tagged-resource 归一化、AND/OR/Not 状态机，以及只暴露 ETag/lock token 的 resolver port。
- 通过 `DavFileSystem` / `DavWriteSystem` / `DavLockSystem` 统一执行 `If`、资源锁、父级锁、父集合存在性与 LOCK lock-null 文件前置条件；产品不再复制 resolver/guard。
- LOCK acquire/refresh 选择、timeout/token/body 校验与成功响应 composition。
- COPY/MOVE/DELETE 的资源路径关系、typed partial failure、207 与 201/204 响应选择。
- 每个标准 DAV 方法的 empty/bounded XML/stream/unused body policy、PATCH format 专属 bounded/stream policy，以及 Actix bounded-body adapter。
- request head 保留规范化后的请求 origin；Actix adapter 按已规划的 body policy 完成 empty/XML/bounded/stream body preparation。
- parsed request target 借用并保留同一个 mount boundary，method parser 使用它校验 `Destination`，调用方不能为目标和目的地传入两套 prefix。
- 通过 `plan_http_conditionals` 统一执行 method-aware HTTP conditional request planning：
  `If-Match`、`If-Unmodified-Since`、`If-None-Match`、GET/HEAD
  `If-Modified-Since`，以及 GET `Range` / `If-Range` 的后续资格。
- HTTP entity-tag 语法、RFC `#rule` 多 field-line 合并、强/弱比较、mapped/unmapped
  资源语义，以及 `304` / `412` 结果选择。
- `DavConditionalPlan::apply_response_headers` 统一规划 `200`、`206`、`304`、`412`、
  `416` 的 `ETag` / `Last-Modified` validator contract。
- GET/HEAD 的 200/206/304/416 response planning、单段 byte range 选择与读取区间。
- PUT 的 `If-Match` / `If-None-Match` 前置条件、`create` / `create_new` 选择、`X-Expected-Entity-Length` 优先级、collection target 405 拒绝和 201/204 成功响应选择。
- `DavRequestHead`、`DavResponse`、`DavEvent` 等协议模型。
- `DavEvent::completed` 从 request head 生成脱敏完成事件；`DavOperationObservations` 只携带可选 bytes、range/resource/backend count、failure class 和 stream outcome，不携带 `If` token、凭据、object key 或正文。
- PROPFIND、PROPPATCH、LOCK、REPORT、VERSION-CONTROL 的 XML 安全校验、QName 语法和未知扩展处理。
- PROPFIND 的 allprop/include/propname/prop selector、去重和 200/404 propstat 分组。
- PROPPATCH 的状态分组、PROPFIND/PROPPATCH XML error mapping、finite-depth 与 207 response composition。
- REPORT 根 QName 解析与 snapshot gate；已实现的 `DAV:version-tree` grammar、file-only mapping、version multistatus 和 VERSION-CONTROL response selection。
- 已知 request grammar 直接遍历 `aster_forge_xml` 的 source-backed arena，不先重复 validation，也不复制整棵通用 DOM；只有需要持久化或回显的 owner/property 子树才物化为 `DavXmlElement`。
- `DavXmlElement` 只承担 DAV 持久化子树与 response composition；通用解析、安全限制、namespace 和 reader/writer 由 `aster_forge_xml` 承担。
- DAV error、multistatus/propstat、dead property、supportedlock/lockdiscovery 和 DeltaV version-tree 的 response grammar。
- 产品中立 backend contracts：`DavFileSystem` / `DavMetaData` 负责单资源机械层，`DavDirectoryEnumerator` 负责 bounded page/cursor 枚举，`DavDownloadSource` 负责 full/range stream，`DavWriteSystem` 负责顺序提交，`DavRandomWriteSystem` 只负责显式随机写，`DavLockSystem` 负责锁持久化与冲突查询。
- `DavLockSystem` 的 discovery、batch discovery 和 conflict lookup 都返回 typed backend failure；产品 adapter 不得把查询失败降级成空锁集合，Forge 会让 `If`、mutation guard 和 `lockdiscovery` fail closed。
- 批量 dead-property 读取只向 backend 传递 `DavPath`；产品 adapter 自行解析数据库身份并执行批量查询。
- Actix transport 与 transport-neutral `http` 类型的显式转换。Actix 仍使用 `http 0.2` 而 Forge 公共模型使用 `http 1.x`，URI/header 跨版本转换保持显式边界。
- Actix adapter 统一完成 header conversion、协议/后端错误响应和 HTTP ETag/`If` guard 映射。
- OPTIONS、405、body-policy failure 和 download response 的 product-neutral response shell。
- resource-aware capability declaration/snapshot、RFC class 与扩展 prerequisite 校验、静态
  package/property/report descriptor、canonical `Allow`/`DAV`/`DASL`/`Accept-Patch` rendering
  和未知 method 的 target-first parsing。
- capability-driven live-property catalog、一次 batch value snapshot、RFC 4331 quota
  allprop/propname/explicit selection，以及 RFC 8144 preference planning。
- RFC 9110 partial PUT 默认拒绝与 typed range plan、RFC 5789 PATCH media-type dispatch，
  以及与二者分离的私有 `X-Update-Range` capability。

## XML 请求语义

所有受信任边界外的 WebDAV XML 先通过同一套大小、深度、DTD/ENTITY 和完整文档校验，再进入方法语义。语义层按 [RFC 4918 Section 17](https://www.rfc-editor.org/rfc/rfc4918.html#section-17) 处理扩展：只识别当前上下文规定的完整 QName；其他元素和属性连同未知元素的完整子树按不存在处理。未知元素即使位于 `DAV:` namespace 也不会因为 namespace 相同而自动成为协议控制，已知名称嵌套在未知子树中也不会被激活。

| 方法 | HTTP body | 根 QName | 已知直接控制 | 顺序与重复 |
| --- | --- | --- | --- | --- |
| PROPFIND | 缺省表示 `allprop`；空白或空根不等于缺省 | `DAV:propfind` | `propname`，`allprop` + 可选 `include`，或 `prop` | 控制顺序无关；selector 互斥，`include` 只能出现一次且必须与 `allprop` 组合 |
| PROPPATCH | 必须存在 | `DAV:propertyupdate` | 有序的 `set` / `remove`，每个 action 恰好一个 `DAV:prop` | action 按文档顺序保留；action 内重复 `prop` 拒绝；至少需要一个有效 property 操作 |
| LOCK acquire | 必须存在 | `DAV:lockinfo` | 一个 `lockscope`、一个 `locktype`、可选一个 `owner` | 控制顺序无关；重复或同时出现多个已知识别值时拒绝 |
| LOCK refresh | 必须缺省 | 无 | token 来自 `If` header | body presence 由 LOCK planner 区分 acquire 与 refresh |
| REPORT `version-tree` | 必须存在 | `DAV:version-tree` | 至多一个直接 `DAV:prop` | 其他元素忽略；重复 `DAV:prop` 拒绝 |
| VERSION-CONTROL | 可缺省；存在时必须是 XML | `DAV:version-control` | RFC 3253 定义为 `ANY` | 安全、完整且根 QName 正确后保留扩展内容 |

结构化协议容器不接受额外字符数据。property-name 上下文只把元素 QName 作为属性名；直接字符数据会被视为非法 property value，而未知子元素仍按 RFC 4918 的完整子树忽略规则处理。`PROPPATCH set` 的 property value 和 LOCK `owner` 属于需要保留的内容，不应用这条 property-name 限制。

DeltaV 语义以 [RFC 3253 Section 3.5](https://www.rfc-editor.org/rfc/rfc3253.html#section-3.5) 和 [Section 3.7](https://www.rfc-editor.org/rfc/rfc3253.html#section-3.7) 为准。`VERSION-CONTROL` 使用 bounded XML body policy，因此 transport adapter 不会绕过产品提供的 XML 请求体上限。

产品负责：

- Basic/WebDAV account 认证与限流。
- principal、个人/团队 workspace scope 和 permission guard。
- 文件、目录、blob、quota、storage policy 和版本业务事务。
- dead property 和 lock 的具体持久化。
- 产品 audit action/detail、metrics label 和用户通知。
- 在 mutation 前提供 writer-authoritative 的 request-target `exists`、`ETag` 和
  `Last-Modified`。Forge 不替产品选择 reader/writer 一致性来源，也不把旧 metadata
  snapshot 当成事务保障。
- 实现 `DavCapabilityProvider` 和所选 package support traits，提供资源类型、权限投影、locking
  事实、当前目标的扩展 package 子集和独立兼容性开关；产品 handler 与响应层只消费 Forge
  snapshot。
- 实现 `DavLivePropertyProvider` 的 batch snapshot，提供请求真正需要的权威值；Forge 不接管
  quota、principal、sync token、dead property 或扩展属性的持久化。

## HTTP 与 WebDAV 条件请求

HTTP conditional planner 以 [RFC 9110 Section 13.2.2](https://www.rfc-editor.org/rfc/rfc9110.html#section-13.2.2)
为执行顺序：

1. `If-Match`，使用 RFC 9110 Section 8.8.3.2 的强比较。
2. 没有 `If-Match` 时执行 `If-Unmodified-Since`。
3. `If-None-Match`，使用弱比较；GET/HEAD 匹配返回 `304`，其他方法返回 `412`。
4. GET/HEAD 且没有 `If-None-Match` 时执行 `If-Modified-Since`。
5. 只有 GET 且前置条件会得到 `200` 时，才继续评估 `Range` / `If-Range`。

日期字段遵循 RFC 9110 Section 13.1.3 和 13.1.4：非法 HTTP-date、日期列表或产品未提供
修改时间时忽略对应日期条件。Entity-tag 列表遵循 RFC 9110 Section 5.6.1.2：接收端容忍
有界数量的空成员；零成员 `If-Match` 条件为假，零成员 `If-None-Match` 条件为真；`*`
与其他 entity-tag 混用仍按 Section 13.1.1/13.1.2 视为非法。

WebDAV `If` 仍是独立的 RFC 4918 Section 10.4 条件。使用 `plan_conditionals` 或
`plan_conditionals_with_backends` 时，Forge 先解析并执行完整 tagged/untagged WebDAV
`If`，成功后再对 request target 执行 HTTP conditional planner。HTTP `If-Match`、
`If-None-Match` 不会被扩展成 Destination-specific header；COPY/MOVE destination 或其他
资源的 ETag/lock token 条件继续由 tagged WebDAV `If` 表达。

## 增量 Multi-Status

`DavMultiStatusWriter<W>` 是 PROPFIND、PROPPATCH、递归 mutation failure 和已支持 REPORT
共用的唯一 `<D:multistatus>` grammar。`dav_multistatus_bytes`、
`property_multistatus_response`、`mutation_multistatus_response` 和 `version_tree_response`
都是该 writer 的完整文档 convenience API，不维护第二套 element/order/namespace 逻辑。

大结果使用 `multistatus_stream_response`，把产品生成的
`Stream<Item = Result<DavMultiStatusItem, DavMultiStatusSourceError>>` 转为
`DavResponseBody::MultiStatus`。writer 逐 item 校验并输出，不要求 `Vec<DavMultiStatusItem>`
或完整 XML `Vec<u8>`；内存只保留当前 typed item、有界 XML writer 状态和有限 chunk 队列。
Actix feature 只把该 typed stream 转为 response body，不把 Actix 类型放进 core。

```rust
let limits = aster_forge_webdav::DavMultiStatusLimits::new(
    16 * 1024 * 1024, // maximum output bytes
    50_000,           // maximum response items
    512,              // maximum properties in one response item
    16 * 1024,        // maximum body chunk bytes
);
let response = aster_forge_webdav::multistatus_stream_response(items, limits)?;
```

`DavMultiStatusError` 保留 item/property/output limit、source backend failure、取消、XML 和
sink write failure 分类，并携带 `DavMultiStatusProgress`。stream 在取得第一个有效 item 前先
轮询 source，因此首项前的 backend failure 或 cancellation 明确标记为
`response_started = false`；一旦 body chunk 已交给 transport，后续错误标记为部分响应，只能
终止 stream。完整文档 helper 尚未交给 transport，调用方仍可在自己的 response boundary 映射
完整错误响应。

## 分页枚举与递归预算

目录遍历使用独立的 `DavDirectoryEnumerator`，不再通过 `DavFileSystem::read_dir` 返回一个可能
在创建 stream 前已经全量收集的伪流。产品 adapter 定义自己的 opaque associated cursor 和
page entry 类型；metadata 随 page entry 一起返回，允许产品批量查询，避免协议 contract 强制
逐 entry metadata N+1。

```rust
let mut state = aster_forge_webdav::DavDirectoryPageState::new();
let limits = aster_forge_webdav::DavDirectoryPageLimits::new(256, 10_000)?;
let page = aster_forge_webdav::read_next_directory_page(
    &enumerator,
    &directory,
    &mut state,
    256,
    limits,
    &cancellation,
)
.await?;
```

pager 在每次 backend call 前检查 `DavCancellation`，并强制：

- requested/page hard limit 和 maximum page count。
- 非空 continuation page。
- cursor 不得在任意后续 page 回环。
- `stable_key` 在页内和跨页严格递增，重复或倒序 page 不进入协议处理。
- invalid page、backend failure 或 cancellation 不推进已验证 cursor state；invalid page 会将
  pager 标记为终态，后续调用复用同一错误，不再重复请求相同 cursor。

pager 为终止性保留至多 `maximum_pages` 个 opaque cursor，并只复制每页最后一个 stable key；
它不会保存全部目录 entry。产品负责数据库 keyset/order contract 和 cursor 编码，Forge 不解析
cursor 内容。

递归 COPY、MOVE、DELETE 或深度 PROPFIND 使用 `DavTraversalBudget` 记录 visited resources、
queued work、failures、maximum depth 和 completed mutations。budget 自身不分配工作队列，也不
执行 mutation；产品持有实际 queue、事务、cleanup 和 side effect。超限或取消返回
`DavTraversalError`，其 progress 和 `partial_execution()` 让调用方区分“尚未修改”与“已部分
执行，需要通过 207/终止语义报告”的结果。

## Backend 与事件

产品应把已认证、已限定 workspace 的 adapter 交给协议层。backend 调用必须同步完成影响协议正确性的操作；quota、blob 引用、lock 持久化和必要的缓存失效不能依赖事件补写。

`DavEventSink` 只观察已经完成的协议操作，适合 tracing、metrics、审计适配和通知。事件使用 transport-neutral `u16` 状态码，不包含请求正文、凭据、lock token 或 object key。

`DavOperationObservations` 中的 `None` 表示未采集，`Some(0)` 表示已采集且计数为零。`DavStreamOutcome` 区分 completed、cancelled、response start 前失败和 response start 后部分传输失败；默认接口不会为每个 chunk 发布事件。

sink 可以返回 `DavObservationError`，但调用边界必须使用 `publish_non_authoritative`。observer 缺失、返回错误或 panic 都会被吞掉，不能改变协议响应、quota、transaction、blob refcount、lock persistence、必要 audit 或缓存正确性。

`DavEventSink::publish` 是同步的快速提交边界，不执行阻塞 I/O。需要异步处理时，产品 sink 必须使用 `try_send` 一类非阻塞操作把事件提交到产品拥有的有界队列，并自行决定队列满时丢弃、合并或计数；Forge 不为每个请求创建线程，也不隐式复制事件。违反该契约的阻塞 sink 会占用当前调用线程。

## 错误边界

- 协议输入错误使用 `DavProtocolError`，由 transport adapter 映射为 WebDAV 状态码和响应。
- 产品 adapter 把业务错误压缩为 `DavBackendErrorKind`；详细错误和产品文案留在产品日志与 API 边界。
- `DavReportPlanError` 区分未知 REPORT QName 与已知但当前资源未开放的 REPORT；产品通过
  `DavReportErrorResponsePolicy` 选择这两类错误的状态码、文案和响应 envelope，XML 错误仍由
  Forge 按协议分类生成响应。
- Forge 不直接返回产品 API envelope，也不依赖产品错误类型。

## 测试要求

- 协议 crate 测试路径逃逸、header grammar、同源 `Destination`、条件请求和 request-head 解析。
- fake backend 矩阵覆盖 ETag + lock token 联合解析、tagged lock root、父级锁、父集合、lock-null 文件创建，以及 metadata/open/finish 错误传播。
- XML 边界矩阵覆盖空体、QName 冲突、未知子树、重复/互斥控制、DTD/ENTITY、reader I/O、输入大小与深度精确临界、非法 UTF-8、转义和大属性值。
- XML response 矩阵覆盖状态行、元素顺序、QName、namespace shadowing/undeclaration、锁字段、死属性重建、异常旧值转义，以及非法 writer model 与深度临界。
- 产品仓库保留真实认证、数据库、存储、quota、audit、能力 provider 和客户端集成测试。
- capability 测试必须覆盖全部 resource state、GET/HEAD 规范化、Class 1/2/3 独立关系、locking、
  22 个 RFC package、DeltaV prerequisite、SEARCH/DASL、兼容性 headers、未知 method，以及
  OPTIONS/405/dispatch/body/property/report 使用同一 snapshot 的一致性。
- live-property 测试必须覆盖 allprop/include/propname/explicit prop、200/404 分组、prefix 保留、
  provider 每资源只调用一次、quota 无限与 collection required value、发现属性生成和畸形产品值。
- 写入能力测试必须覆盖 partial PUT 默认拒绝、Content-Range 全部边界、强 If-Match、
  PATCH media-type 与 body policy dispatch、Accept-Patch、私有 header 冲突及实际 stream
  长度由产品提交边界核验。
- backend port 测试必须覆盖 full/range exact length、open failure、length drift、empty body 不打开 stream、顺序 finish/abort 和显式 random write。
- observation 测试必须覆盖所有可选计数、未采集与零的区别、cancellation、response-started failure，以及 observer 缺失/失败不影响完成结果。
- Multi-Status 测试必须覆盖 property/status grammar、item/property/byte/chunk 精确边界、空文档、
  source backend failure、首项前后 cancellation、sink 在首字节前后失败，以及 Actix stream
  conversion。
- directory pager 测试必须覆盖空 final page、非空 continuation、cursor cycle、页内与跨页重复/
  倒序、page hard limit、backend failure，以及取消后不再请求下一页。
- traversal budget 测试必须覆盖 visited/work/failure/depth 的精确上限和超限、取消、partial
  execution progress 与无额外 work storage 的值语义。
- Litmus、rclone、curl、cadaver 兼容测试仍应针对具体产品 server 运行，因为它们验证的是协议层和产品 adapter 的组合结果。

## 参考项目

- AsterDrive：`src/webdav/` 保留产品 adapter；`tests/webdav/` 和 WebDAV compatibility workflow 验证完整产品行为。
