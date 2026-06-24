# aster_forge_crypto

`aster_forge_crypto` 收纳跨项目共享的密码哈希和摘要工具。当前模块集中在 `hash`。

## 适用场景

- Argon2 密码哈希。
- 密码校验。
- SHA-256 digest 和 hex 编码。
- 创建可流式更新的 SHA-256 hasher。

不适合放在这里的内容：

- 产品密码策略。
- 登录失败锁定。
- 密码重置 token。
- 加密密钥管理。

## Cargo 接入

```toml
[dependencies]
aster_forge_crypto = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## 密码哈希

```rust
use aster_forge_crypto::{hash_password, verify_password};

let hash = hash_password(password)?;
let ok = verify_password(password, &hash)?;
```

返回错误类型是 `CryptoError`。产品侧应该把它映射成内部错误，不要把底层哈希失败细节直接暴露给用户。

## SHA-256

常用 API：

- `sha256_hex(data)`
- `bytes_to_hex(bytes)`
- `sha256_digest_to_hex(digest)`
- `new_sha256()`

`new_sha256()` 适合文件上传、流式读取或对象存储校验场景。产品侧仍然负责读取 chunk、处理 IO 错误和决定 hash 字段如何持久化。

## 接入边界

密码策略应该留在产品层，例如：

- 最小长度。
- 是否允许弱密码。
- 是否需要旧 hash 迁移。
- 登录失败提示。

Forge 只保证同一套哈希实现被多个项目复用。

## 测试要求

- 同一密码能通过 `verify_password`。
- 错误密码校验失败。
- SHA-256 输出与固定向量一致。
- 产品侧密码策略测试不要搬到 Forge。

## 参考项目

- AsterDrive：分享密码、用户认证密码可以参考此 crate 的接入方式。
- AsterYggdrasil：用户认证路径适合看错误映射如何保留产品文案。
