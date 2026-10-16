# aster_forge_auth

本 crate 拥有产品无关的本地网页会话机制：严格 JWT 签发/验证、Subject/SessionId/RefreshJti/SessionVersion 类型、refresh 状态决策、有界实例协调和事务感知的存储 port。

账号、租户、权限、MFA/step-up、数据库实体/migration、事务、Cookie/CSRF、缓存、审计和最终 HTTP 错误由产品拥有。它不是 OAuth/OIDC Authorization Server，也不属于上游认证客户端 `external_auth` 或加密原语 `crypto`。

## 适用场景

- Drive 的整数账号与 Gate 的 UUID 账号共用 token/refresh 机制，使用独立签名配置和命名空间。
- 保留服务端会话、账号 generation 和单次 refresh CAS；不为省一次数据库查询放宽撤销或权限承诺。
- 同进程竞争与跨实例竞争使用同一套拒绝/提交契约。

## Cargo 与 Features

```toml
[dependencies]
aster_forge_auth = { git = "https://github.com/AsterCommunity/AsterForge" }
```

可选 `password` feature 提供实例级 `PasswordHashRuntime`：同步密码学原语仍在 crypto，blocking 执行和 semaphore 成本控制由 auth 提供；产品选择并发上限、使用等待或 `try_verify_password` 立即拒绝、凭据来源和错误映射。取消请求不会提前释放仍在执行的 blocking job 额度。

默认内核不引入 HTTP 框架、SeaORM、Redis、OpenAPI 或产品实体。JWT 使用成熟的 `jsonwebtoken` / `aws_lc_rs` 后端，默认 HS256，显式支持 HS384/HS512。HMAC key 至少分别为 32/48/64 字节；长度检查不等于熵检查。产品加载高熵签名 key，负责轮换和用途隔离，不复用 AEAD master key。

## 最小接入

```rust
use aster_forge_auth::{AuthTokenType, JwtKey, JwtPolicy, JwtSigner, JwtVerifier,
    Restriction, SessionId, SessionVersion, Subject};

fn example(signing_key_bytes: &[u8]) -> aster_forge_auth::Result<()> {
    let key = JwtKey::hmac(signing_key_bytes)?;
    let policy = JwtPolicy::new("aster-drive", "aster-drive-web", 900, 604_800)?;
    let signer = JwtSigner::with_system_clock(key.clone(), policy.clone());
    let verifier = JwtVerifier::with_system_clock(key, policy);
    let pair = signer.issue(Subject::new("42")?, SessionId::new("session-1")?,
        SessionVersion::new(1), Restriction::None)?;
    let verified = verifier.verify(&pair.access_token, AuthTokenType::Access)?;
    assert_eq!(verified.claims().sub.as_str(), "42");
    // 产品先提交会话创建，再发送 pair；鉴权还需要当前账号状态和有效授权。
    Ok(())
}
```

同样的 rustdoc 示例会执行 doctest。完整可编译的签发、认证、refresh、撤销和关闭 fixture 位于 `crates/aster_forge_auth/examples/session_flow.rs`：

```bash
cargo run -p aster_forge_auth --example session_flow
```

JWT 必需 `sub/sid/session_version/token_type/restriction/iat/exp/iss/aud`；refresh 还必需非空 JTI，access 不允许携带 JTI。`nbf` 可选。验证要求明确的预期 token 用途，算法、issuer 和 audience 不可由客户端 header 选择；不提供禁用 namespace/session 校验的兼容开关。默认最大 token 16 KiB，在解析 header 前检查。时间和 ID 来源可注入，时间转换/溢出和标识符约束均显式检查。

## 运行时资源

产品初始化一个 `RefreshCoordinator`，与 HTTP 实例一起持有，所有 request-scoped service 使用它的 clone；TTL 热更新可重新构建 signer/verifier，但不能每次重建锁表。默认请求预算 1024，包含执行中和等待同 JTI 的请求；超限立即拒绝。正常返回、store 错误、等待取消和 future drop 都释放 permit 和 JTI entry。

`close()` 停止接纳新请求并唤醒等待者。产品在 HTTP drain 后关闭 writer；已经进入 store 的事务允许完成。显式取消只在等待和进入 persistence 前生效，避免把已提交结果错误描述成未执行。如果调用方直接丢弃 future，store 必须保证未提交事务 rollback-on-drop；提交后响应丢失的旧 refresh 仍然不能再成功。

## Store 与事务契约

`RefreshSessionStore::Error` 是产品原始错误，通过 `RefreshError::Store(error)` 原样返回。两个 port 分工如下：

| Port | 产品执行内容 | 返回条件 |
| --- | --- | --- |
| `rotate_refresh` | 同事务检查账号、version、restriction、sid、撤销和 session 到期；调用 `classify_refresh`；执行 CAS 或重放撤销及必要审计 | 提交后才能返回 `Rotated` / `Rejected` |
| `resolve_conflict` | CAS 失败后新建 writer 事务，重读赢家的提交状态，重新校验并分类；必要时同事务撤销与审计 | 只返回拒绝，不能第二次旋转或发送输家的候选 token |

鉴权拒绝与基础设施失败是两回事。`Ok(Rejected(ReuseDetected))` 意味着必要安全动作已经提交，HTTP 401 不能把撤销回滚；DB、audit、CAS 或 commit 失败是 `Err`，没有可用新 token，也不能宣称撤销成功。提交后的缓存/通知失败与事务失败分开处理；权限检查不能被旧缓存回填恢复。

`current_refresh_valid_until` 保存验证时的 `exp + leeway`，使用宽整数防止溢出。产品取得数据库锁后必须从可信时钟更新请求的 `now` 再分类；数据库锁等待不能延长凭据有效期。Drive 使用取得账号锁后的时间，已有过期凭据不得触发旋转或重放撤销。

`RefreshPolicy::Strict` 是默认。产品可以显式选择 `SameClientWithin { seconds }`：只有同账号、同 sid、活跃且未过期 session 的立即前一枚 JTI，在非负时间窗口内、非空 UA 匹配且 IP 不冲突，才分类为 stale。stale 始终拒绝且没有新 token；窗口不是宽限授权。UA/IP 可伪造，只是产品接受风险后的拒绝分类信号。更旧的 replay 根据签名 sid 定位活跃 session 后仍分类 reuse；跨账号/会话、缺失和撤销状态不触发别的账号撤销。

## 授权与错误边界

`VerifiedToken` 只证明 JWT 验证通过，不能证明账号仍活跃或权限仍有效。产品仍须校验当前账号状态、generation、登录 restriction，以及敏感操作的实时权限/MFA。

单会话立即撤销要求 `sid + 权威 session 状态检查`；只携带 sid 不足以兑现承诺。Drive 当前明确保留单会话撤销后的短期 access 到期窗口，同时账号级 generation/禁用/权限检查读取 writer，不依赖可能迟到的缓存。Gate 接入时必须保留其立即撤销和高危实时权限要求。

`AuthError` 只描述机制失败，产品选择状态码、文案和审计 action。token、key 和 JTI 不出现在 Debug/Display。产品审计使用稳定 session/actor 标识，不能记录 refresh 凭据或 JTI。

## 测试要求

- JWT 的必需字段、类型、空/超长/非法 ID、错 key/算法/用途/issuer/audience、未来/过期/nbf/时间溢出、畸形/超大 token、密钥长度和 redaction。
- 同实例真实等待、不同实例 CAS、重启后多轮历史 replay；旧 token 没有第二个成功响应。
- 显式 stale/reuse 策略、窗口边界、伪造/缺失指纹、跨账号/会话、撤销/过期/禁用/version/restriction。
- begin/CAS/revoke/audit/commit/重读失败，未提交状态回滚、无 token 泄漏，post-commit 事实不回滚。
- 资源超限、锁回收、排队到期、显式取消、future drop、shutdown 与实例隔离，使用 gate/barrier/注入时钟而非随意 sleep。
- Drive 的真实 router + SQLite 回归、CAS/audit 触发器与延迟外键 commit 故障注入、缓存删除失败和旧缓存回填；PG/MySQL 接入使用产品测试的 `ASTER_TEST_DATABASE_BACKEND`。SQLite 专用故障注入只在 SQLite 执行，其余适用用例验证三种后端。

```bash
cargo test -p aster_forge_auth --all-targets
cargo test -p aster_forge_auth --doc
cargo clippy -p aster_forge_auth --all-targets -- -D warnings
cargo check --workspace
cargo test --workspace
```

## 参考项目

| 原模块 | Forge 机制 | 产品保留责任 | 行为测试 |
| --- | --- | --- | --- |
| Drive JWT encode/decode | `JwtSigner` / `JwtVerifier` / typed claims | key 配置、TTL、整数账号转换、restriction 及实时授权 | namespace/type/时间/旧格式拒绝、登录及受限登录 |
| Drive 全局 JTI 锁与重复 refresh 流程 | `RefreshCoordinator` / `RefreshTokenService` / `classify_refresh` | 实例初始化、关闭、明确 stale 策略、HTTP 错误与指标 | 真正等待、多实例 CAS、取消、超限、回收、历史 replay |
| Drive session 轮换 SQL 与 reuse 副作用 | `RefreshSessionStore` 提交契约 | writer 事务、账号状态、条件 SQL、版本递增、purge、安全审计、缓存失效 | CAS/revoke/audit/commit 故障回滚、跨账号/会话 fence、旧缓存回填 |
| Drive Cookie 与 session 管理 | JWT 的 sid/version/restriction 契约 | Cookie/CSRF、会话列表与撤销、access 撤销策略 | router、HttpOnly/Path/CSRF、单会话窗口、账号级撤销 |

AsterDrive 的 `services/auth/local/tokens` 保存产品 policy/claims projection，`refresh` 实现事务 port，`db/repository` 保存 SQL 和实体；原始全局 JTI 锁、测试 hook 和重复 refresh 流程已经移除。Drive 旧 JWT 不兼容，升级后用户重新登录；Cookie、DTO 和 HTTP transport 不变，签发与刷新都读取同一份当前产品 TTL。

Drive 账号 generation 使用同事务的原子递增，改密、账号级撤销、MFA 重置、管理员状态修改和 reuse 不再覆盖并发版本。负值或 `i64::MAX` 视为不可继续递增，返回基础设施失败并回滚，不能饱和后仍宣称旧 access 已撤销。持久化的亚秒级 `last_seen_at` 不因秒级 JWT 时间倒退；缺失 UA/IP 保留历史字段，但 replay 决策只使用本次请求证据。

AsterGate 后续使用 UUID subject 和独立 issuer/audience/key，保留自己的实体、即时撤销、system permissions、step-up 和 audit；不能照搬 Drive 的 access 到期窗口。
