# aster_forge_crypto

`aster_forge_crypto` 收纳跨项目共享的密码哈希、带密钥消息认证和摘要工具。当前模块集中在 `hash`。

## 适用场景

- Argon2 密码哈希。
- 密码校验。
- 密码哈希工作因子、验证资源上限和渐进重哈希判断。
- SHA-256 digest 和 hex 编码。
- HMAC-SHA-256 keyed digest。
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

默认策略显式固定为 RFC 9106 第二推荐项：

```text
algorithm = Argon2id
version = 19
memory = 64 MiB
iterations = 3
parallelism = 4
output = 32 bytes
salt = 16 random bytes
```

默认验证资源上限与默认工作因子相同。旧的 Forge 默认值
`m=19456 KiB, t=2, p=1` 仍可验证，并会被标记为需要重哈希；超过验证上限的 PHC
参数会在 Argon2 分配工作内存前被拒绝。

返回错误类型是 `CryptoError`。密码不匹配返回 `Ok(false)`；格式错误、不支持的算法或版本、非法参数和超出资源上限返回错误。产品侧应该把错误映射成内部错误，不要把底层失败细节直接暴露给用户。

## 自定义工作因子与验证上限

新哈希参数和验证上限是两个独立边界：

```rust
use aster_forge_crypto::{
    PasswordHashPolicy, PasswordHashVerificationLimits, PasswordHashWorkFactor,
    hash_password_with_policy, verify_password_with_policy,
};

let work_factor = PasswordHashWorkFactor::new(
    96 * 1024, // memory KiB
    3,         // iterations
    4,         // parallelism
    32,        // output bytes
)?;
let limits = PasswordHashVerificationLimits::new(
    128 * 1024,
    5,
    8,
    64,
)?;
let policy = PasswordHashPolicy::new(work_factor, limits)?;

let hash = hash_password_with_policy(password, &policy)?;
let verification = verify_password_with_policy(password, &hash, &policy)?;
if verification.is_valid && verification.needs_rehash {
    // 产品侧在自己的事务、审计和错误边界内更新持久化 hash。
}
```

`PasswordHashPolicy::new` 会拒绝“新哈希工作因子高于自身验证上限”的无效组合。验证上限应该按单实例内存预算、认证并发限制和实际部署硬件设置，而不是只看单次 benchmark。

Argon2 是同步 CPU/内存工作。Actix/Tokio 产品应该在产品运行时边界控制认证速率和并发；提高工作因子时，同时核对 blocking worker、semaphore 和实例内存预算。

## SHA-256

常用 API：

- `sha256_hex(data)`
- `bytes_to_hex(bytes)`
- `sha256_digest_to_hex(digest)`
- `new_sha256()`

`new_sha256()` 适合文件上传、流式读取或对象存储校验场景。产品侧仍然负责读取 chunk、处理 IO 错误和决定 hash 字段如何持久化。

裸 SHA-256 适合内容摘要和高熵随机 token 的查找摘要，不适合密码、短验证码、PIN、低熵 recovery code 或需要防篡改的消息。

需要带密钥的稳定摘要时使用 HMAC-SHA-256：

```rust
use aster_forge_crypto::hmac_sha256_hex;

let cache_component = hmac_sha256_hex(cache_key_secret, credential.as_bytes())?;
```

产品负责提供高熵、用途隔离的 key，并负责 key 的加载、轮换和持久化策略。HMAC-SHA-256 不替代人类密码的 Argon2id。

## 接入边界

密码策略应该留在产品层，例如：

- 最小长度。
- 是否允许弱密码。
- 是否需要旧 hash 迁移。
- 登录失败提示。
- HMAC key 的来源、用途隔离和轮换。
- Argon2 的认证并发上限和 blocking 执行策略。

Forge 只保证同一套哈希实现被多个项目复用。

## 测试要求

- 同一密码能通过 `verify_password`。
- 错误密码校验失败。
- 默认 hash 明确编码 RFC 9106 第二推荐参数。
- 旧工作因子验证成功并返回 `needs_rehash`。
- 超过验证上限的 `m`、`t`、`p` 和输出长度在计算前被拒绝。
- 不支持的算法、版本和损坏 PHC 返回错误，而不是伪装成密码不匹配。
- SHA-256 输出与固定向量一致。
- HMAC-SHA-256 输出与 RFC 4231 固定向量一致。
- 产品侧密码策略测试不要搬到 Forge。

## 参考项目

- AsterDrive：分享密码、用户认证密码可以参考此 crate 的接入方式。
- AsterYggdrasil：用户认证路径适合看错误映射如何保留产品文案。
