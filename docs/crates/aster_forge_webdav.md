# aster_forge_webdav

`aster_forge_webdav` 是 Aster 产品共用的 WebDAV 协议边界。它负责把 HTTP/WebDAV 输入解析为强类型请求，校验路径、`Depth`、`Destination`、`If`、ETag 和日期条件，并定义协议响应、产品 backend port 与操作事件。

这个 crate 不拥有 AsterDrive 的文件业务。认证账号、workspace scope、权限、SeaORM entity、存储策略、quota、版本落库和审计展示仍然留在产品仓库。

## Cargo 接入

```toml
[dependencies]
aster_forge_webdav = { git = "https://github.com/AsterCommunity/AsterForge", features = ["actix"] }
```

默认 feature 只包含 transport-neutral 协议内核。Actix 产品启用 `actix`，使用 `aster_forge_webdav::actix` 完成请求和响应类型转换。

## 能力声明与响应快照

产品通过 `DavCapabilityProvider` 提供类型化能力声明。产品可以在自己的类型上组合读取、写入、属性、锁和版本能力 impl，最终由一个 provider 根据 `DavCapabilityTarget` 和 `DavCapabilityContext` 投影当前请求的资源能力：

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

Rust 没有运行时 trait 反射，Forge 通过显式的 `DavCapabilityProvider` 聚合产品 impl；Forge 不扫描 handler，也不从字符串推测能力。provider 可以把资源类型、mount root、principal、权限、锁配置和 DeltaV 开关合并成 `DavCapabilityDeclaration`。`plan_capabilities` 校验声明并生成不可变的 `DavCapabilitySnapshot`。

静态能力通过 associated profile 进一步约束：`DavClass2Profile` 要求 provider 实现 `DavClass2Support`，而该 trait 继承 `DavClass1Support`；版本能力使用对应的 `DavClass1VersioningProfile` / `DavClass2VersioningProfile`。因此 class 2 缺少 class 1、版本 profile 缺少基础 profile 等结构性错误会在编译期报错。请求目标的存在性、file/collection 类型、principal 权限和运行时开关仍属于动态投影，由 planner 在请求时验证。

provider trait 使用原生 `async fn` 和泛型静态分发，不为 capability lookup 引入 `Pin<Box<dyn Future>>` 堆分配。405 gate 只返回零大小的 typed error，实际响应由同一个 snapshot 构造。

同一个 snapshot 必须被 OPTIONS、405、实际 method dispatch、body policy 和扩展 discovery 共用。`gate_method` 负责把已知或未知 method 统一接入 405 gate。产品不直接拼接 `Allow`、`DAV` 或 `version-control` header。

`DavLockingCapability::Class2` 只有在 class 1、LOCK 和 UNLOCK 同时成立时才会通过规划；`DavVersioningCapability::Core` 只有在 class 1、REPORT 和 VERSION-CONTROL 同时成立时才会生成 `version-control` token。`MS-Author-Via` 作为独立兼容性 flag，不进入 DAV compliance token。

## 协议所有权

Forge 负责：

- `DavPath` 的百分号解码、dot-segment 规范化和 mount escape 拒绝。
- WebDAV 方法、`Depth`、`Overwrite`、`Destination`、`If`、`Timeout` 和 `Lock-Token` header 解析。
- `If` tagged-resource 归一化、AND/OR/Not 状态机，以及只暴露 ETag/lock token 的 resolver port。
- 通过 `DavFileSystem` / `DavLockSystem` 统一执行 `If`、资源锁、父级锁、父集合存在性与 LOCK lock-null 文件前置条件；产品不再复制 resolver/guard。
- LOCK acquire/refresh 选择、timeout/token/body 校验与成功响应 composition。
- COPY/MOVE/DELETE 的资源路径关系、typed partial failure、207 与 201/204 响应选择。
- 每个 DAV 方法的 empty/bounded XML/stream/unused body policy，以及 Actix bounded-body adapter。
- request head 保留规范化后的请求 origin；Actix adapter 按方法一次性完成 empty/XML/stream body preparation。
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
- `DavEvent::completed` 从 request head 生成脱敏完成事件，不携带 `If` token、凭据或正文。
- PROPFIND、PROPPATCH、LOCK、REPORT、VERSION-CONTROL 的 XML 安全校验、QName 语法和未知扩展处理。
- PROPFIND 的 allprop/include/propname/prop selector、去重和 200/404 propstat 分组。
- PROPPATCH 的状态分组、PROPFIND/PROPPATCH XML error mapping、finite-depth 与 207 response composition。
- DeltaV `DAV:version-tree` REPORT 选择、file-only/unsupported mapping、version multistatus 和 VERSION-CONTROL response selection。
- 已知 request grammar 直接遍历 `aster_forge_xml` 的 source-backed arena，不先重复 validation，也不复制整棵通用 DOM；只有需要持久化或回显的 owner/property 子树才物化为 `DavXmlElement`。
- `DavXmlElement` 只承担 DAV 持久化子树与 response composition；通用解析、安全限制、namespace 和 reader/writer 由 `aster_forge_xml` 承担。
- DAV error、multistatus/propstat、dead property、supportedlock/lockdiscovery 和 DeltaV version-tree 的 response grammar。
- 唯一 backend contract：`DavFileSystem`、`DavMetaData`、`DavFile`、`DavDirEntry`、
  `DavLockSystem`、`FsError` 和 `OpenOptions`；产品只实现这些 Forge port，不再复制协议 trait。
- 批量 dead-property 读取只向 backend 传递 `DavPath`；产品 adapter 自行解析数据库身份并执行批量查询。
- Actix transport 与 transport-neutral `http` 类型的显式转换。
- Actix adapter 统一完成 header conversion、协议/后端错误响应和 HTTP ETag/`If` guard 映射。
- OPTIONS、405、body-policy failure 和 download response 的 product-neutral response shell。
- resource-aware capability declaration/snapshot、RFC 4918 class dependency validation、
  canonical `Allow`/`DAV` rendering 和未知 method 的 target-first parsing。

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
- 实现 `DavCapabilityProvider`，提供资源类型、权限投影、locking 事实、DeltaV 状态和
  独立兼容性开关；产品 handler 与响应层只消费 Forge snapshot。

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

## Backend 与事件

产品应把已认证、已限定 workspace 的 adapter 交给协议层。backend 调用必须同步完成影响协议正确性的操作；quota、blob 引用、lock 持久化和必要的缓存失效不能依赖事件补写。

`DavEventSink` 只观察已经完成的协议操作，适合 tracing、metrics、审计适配和通知。事件使用 transport-neutral `u16` 状态码，不包含请求正文、凭据或 lock token。

## 错误边界

- 协议输入错误使用 `DavProtocolError`，由 transport adapter 映射为 WebDAV 状态码和响应。
- 产品 adapter 把业务错误压缩为 `DavBackendErrorKind`；详细错误和产品文案留在产品日志与 API 边界。
- Forge 不直接返回 AsterDrive 的 envelope，也不依赖产品错误类型。

## 测试要求

- 协议 crate 测试路径逃逸、header grammar、同源 `Destination`、条件请求和 request-head 解析。
- fake backend 矩阵覆盖 ETag + lock token 联合解析、tagged lock root、父级锁、父集合、lock-null 文件创建，以及 metadata/open/flush 错误传播。
- XML 边界矩阵覆盖空体、QName 冲突、未知子树、重复/互斥控制、DTD/ENTITY、reader I/O、输入大小与深度精确临界、非法 UTF-8、转义和大属性值。
- XML response 矩阵覆盖状态行、元素顺序、QName、namespace shadowing/undeclaration、锁字段、死属性重建、异常旧值转义，以及非法 writer model 与深度临界。
- 产品仓库保留真实认证、数据库、存储、quota、audit、能力 provider 和客户端集成测试。
- capability 测试必须覆盖 unmapped、file、collection、mount root、GET/HEAD 规范化、
  class 1/2、locking on/off、DeltaV on/off、兼容性 headers、未知 method，以及
  OPTIONS/405/dispatch 使用同一 snapshot 的一致性。
- Litmus、rclone、curl、cadaver 兼容测试仍应针对具体产品 server 运行，因为它们验证的是协议层和产品 adapter 的组合结果。

## 参考项目

- AsterDrive：`src/webdav/` 保留产品 adapter；`tests/webdav/` 和 WebDAV compatibility workflow 验证完整产品行为。
