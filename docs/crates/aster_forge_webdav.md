# aster_forge_webdav

`aster_forge_webdav` 是 Aster 产品共用的 WebDAV 协议边界。它负责把 HTTP/WebDAV 输入解析为强类型请求，校验路径、`Depth`、`Destination`、`If`、ETag 和日期条件，并定义协议响应、产品 backend port 与操作事件。

这个 crate 不拥有 AsterDrive 的文件业务。认证账号、workspace scope、权限、SeaORM entity、存储策略、quota、版本落库和审计展示仍然留在产品仓库。

## Cargo 接入

```toml
[dependencies]
aster_forge_webdav = { git = "https://github.com/AsterCommunity/AsterForge", features = ["actix"] }
```

默认 feature 只包含 transport-neutral 协议内核。Actix 产品启用 `actix`，使用 `aster_forge_webdav::actix` 完成请求和响应类型转换。

## 协议所有权

Forge 负责：

- `DavPath` 的百分号解码、dot-segment 规范化和 mount escape 拒绝。
- WebDAV 方法、`Depth`、`Overwrite`、`Destination`、`If`、`Timeout` 和 `Lock-Token` header 解析。
- `If` tagged-resource 归一化、AND/OR/Not 状态机，以及只暴露 ETag/lock token 的 resolver port。
- LOCK acquire/refresh 选择、timeout/token/body 校验与成功响应 composition。
- COPY/MOVE/DELETE 的资源路径关系、typed partial failure、207 与 201/204 响应选择。
- 每个 DAV 方法的 empty/bounded XML/stream/unused body policy，以及 Actix bounded-body adapter。
- request head 保留规范化后的请求 origin；Actix adapter 按方法一次性完成 empty/XML/stream body preparation。
- HTTP ETag、`If-Modified-Since`、`If-Unmodified-Since` 的协议优先级。
- GET/HEAD 的 200/206/304/416 response planning、单段 byte range 选择与读取区间。
- `DavRequestHead`、`DavResponse`、`DavEvent` 等协议模型。
- PROPFIND、PROPPATCH、LOCK、REPORT 的 XML 安全校验、QName 语法和未知扩展处理。
- PROPFIND 的 allprop/include/propname/prop selector、去重和 200/404 propstat 分组。
- PROPPATCH 的状态分组、PROPFIND/PROPPATCH XML error mapping、finite-depth 与 207 response composition。
- DeltaV `DAV:version-tree` REPORT 选择、file-only/unsupported mapping、version multistatus 和 VERSION-CONTROL response selection。
- `DavXmlElement` XML 表示与序列化边界；具体 XML crate 是 Forge 私有实现，产品不直接依赖。
- DAV error、multistatus/propstat、dead property、supportedlock/lockdiscovery 和 DeltaV version-tree 的 response grammar。
- `DavResourceBackend`、`DavPropertyBackend`、`DavLockBackend` 和可选 `DavVersionBackend` port。
- Actix transport 与 transport-neutral `http` 类型的显式转换。
- OPTIONS、405、body-policy failure 和 download response 的 product-neutral response shell。

产品负责：

- Basic/WebDAV account 认证与限流。
- principal、个人/团队 workspace scope 和 permission guard。
- 文件、目录、blob、quota、storage policy 和版本业务事务。
- dead property 和 lock 的具体持久化。
- 产品 audit action/detail、metrics label 和用户通知。

## Backend 与事件

产品应把已认证、已限定 workspace 的 adapter 交给协议层。backend 调用必须同步完成影响协议正确性的操作；quota、blob 引用、lock 持久化和必要的缓存失效不能依赖事件补写。

`DavEventSink` 只观察已经完成的协议操作，适合 tracing、metrics、审计适配和通知。事件使用 transport-neutral `u16` 状态码，不包含请求正文、凭据或 lock token。

## 错误边界

- 协议输入错误使用 `DavProtocolError`，由 transport adapter 映射为 WebDAV 状态码和响应。
- 产品 adapter 把业务错误压缩为 `DavBackendErrorKind`；详细错误和产品文案留在产品日志与 API 边界。
- Forge 不直接返回 AsterDrive 的 envelope，也不依赖产品错误类型。

## 测试要求

- 协议 crate 测试路径逃逸、header grammar、同源 `Destination`、条件请求和 request-head 解析。
- XML 边界矩阵覆盖空体、QName 冲突、未知子树、重复/互斥控制、DTD/ENTITY、深度临界、UTF-8、转义和大属性值。
- XML response 矩阵覆盖状态行、元素顺序、QName、命名空间声明、锁字段、死属性重建和异常旧值转义。
- 产品仓库保留真实认证、数据库、存储、quota、audit 和客户端集成测试。
- Litmus、rclone、curl、cadaver 兼容测试仍应针对具体产品 server 运行，因为它们验证的是协议层和产品 adapter 的组合结果。

## 参考项目

- AsterDrive：`src/webdav/` 保留产品 adapter；`tests/webdav/` 和 WebDAV compatibility workflow 验证完整产品行为。
