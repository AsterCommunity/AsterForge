# `aster_forge_auth`

本 crate 只负责产品无关的本地会话机制：JWT 签发与校验、typed claims、refresh 单次轮换协调和产品存储 port。

产品负责账号状态、权限、Cookie/Authorization transport、数据库实体与事务、缓存、审计、密钥加载/轮换和 API 错误映射。不要在这里引入 SeaORM、Actix、产品实体或业务权限。

## 验证

```bash
cargo test -p aster_forge_auth
cargo test -p aster_forge_auth --all-targets
cargo test -p aster_forge_auth --doc
cargo clippy -p aster_forge_auth --all-targets -- -D warnings
```

refresh store 必须证明 CAS 是跨实例事实源；`RefreshCoordinator` 由产品实例持有，包含有界请求预算、取消和回收，不能每次请求重建，也不能替代数据库 CAS。

`rotate_refresh` / `resolve_conflict` 必须在产品事务里重新校验账号/version/restriction 和 sid。reuse 分类、必要撤销和必要审计同事务提交，再返回 typed rejection；不要通过 `Err` 认证拒绝把已安排的撤销回滚。产品错误通过 associated `Error` 原样返回。

禁止为了兼容旧 JWT 增加禁用 issuer/audience/sid 的开关或双来源 claims。公开 API 的注释应说明错误、提交、取消及授权边界，缺少文档会触发 `missing_docs`。
