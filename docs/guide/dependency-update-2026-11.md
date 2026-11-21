# 2026-11 依赖维护与 time 迁移

本次为尚未发布的源码变更，公开类型迁移说明以各 crate 文档为准。

- `cargo upgrade --incompatible allow --pinned allow --recursive false` 和 `cargo update` 均已实际执行；fuser 0.18 → 0.19，Actix Web 4.19 → 4.20，lettre 0.11.23 → 0.11.24，SQLx 0.9.0 → 0.9.1、SQLite 原生库 0.37 → 0.40。
- 依赖版本、features 和来源在根 workspace；成员继承，不在成员中固定第三方版本。模板和 release 工具有自己的独立 workspace。
- Forge 的直接 chrono 使用迁到 time；没有引入 jiff 作为 Forge 直接依赖。SQLx 的传递依赖仍可包含 jiff，OIDC/OAuth2 与 SeaORM migration 的发布版仍可带入 chrono。`cargo audit` 仍报告 chrono 停维护，不能称为整个 lockfile 已消除；没有新增告警忽略。
- 删除 `.cargo/audit.toml` 中 SQLite 和 crypto-bigint 两条例外。保留四条：RSA 0.9 受 OIDC 版本范围约束；h2 0.3.27 受 Actix HTTP 3.22 约束；paste 仅 Linux allocator 编译期；astral-tokio-tar 仅测试容器。例外保留残留风险，不代表修复。
- 仅剩的可见落后项是传递约束：crypto-common 0.1.7 精确要求 generic-array 0.14.7，Axum 0.8.9 精确要求 matchit 0.8.4；不强行破坏依赖约束。
- audit workflow 每天 09:17 UTC（北京时间 17:17）执行。

## 消费者迁移

游标、task、runtime lease、DB store 的 `DateTime<Utc>` 改为 `time::OffsetDateTime`，产品 trait 实现、DTO、查询参数、测试和自定义模型需要一起迁移；未提供旧类型别名。`Utc::now()` 对应 `OffsetDateTime::now_utc()`，chrono duration 对应 time duration。数据库实体使用 `TimeDateTimeWithTimeZone`，现有列与 fence 谓词保留。JSON 仍使用 RFC3339 字符串/null，产品 DTO 需要显式 serde RFC3339 模块，避免默认 tuple。Time 默认支持 -9999 至 9999 年，饱和租约采用该范围。

## 独立版本递增

以下版本表示本次发布意图；crate 发行状态以 registry 和审查后的 release plan 为准。依赖变化通过第三方 lockfile 可达图识别，再覆盖所有 optional/dev/target 反向消费者；不发布独立 fuzz fixture。无变化的 macros、API-doc macros、validation 不升版。

| crate | 原版本 | 本次版本 |
| --- | --- | --- |
| `aster_forge_alloc` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_api` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_audit` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_auth` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_cache` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_cloud_files_core` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_cloud_files_linux` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_cloud_files_macos_bridge` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_cloud_files_windows` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_config` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_crypto` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_db` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_db_migration` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_events` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_external_auth` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_file_classification` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_http` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_logging` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_mail` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_metrics` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_middleware` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_observability` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_panic` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_process` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_runtime` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_storage_core` | 0.1.0-alpha.1 | 0.1.0-alpha.2 |
| `aster_forge_tasks` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_test` | 0.1.0-alpha.3 | 0.1.0-alpha.4 |
| `aster_forge_utils` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_webdav` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |
| `aster_forge_xml` | 0.1.0-alpha.2 | 0.1.0-alpha.3 |

## 本地验证与已知缺口

环境为 macOS arm64；Linux 使用 Docker Alpine 3.24 / Rust 1.100 原生容器。declared MSRV Rust 1.95 的 workspace all-targets check 与 panic Clippy 通过。stable workspace all-targets Clippy、默认 workspace tests、DB 全业务 features、tasks/runtime 全 features、API OpenAPI、audit DB writer/lifecycle 均验证。新增三库真实时间 round-trip 与到期接管 fence 测试；SMTP 六条漏洞对应的 message/EHLO/loopback 回归全部通过，包括超限未终止响应、STARTTLS 回复后注入、裸 CR/LF 与分块 dot-stuffing。

Linux cloud-files/panic 的 all-targets 测试和严格 Clippy 通过。真实 FUSE 挂载验证了读写、truncate、fsync/reopen、独占创建、并发定位写入、mkdir、跨目录 rename、稳定 inode、NOREPLACE 与错误映射。覆盖已有目标的 rename 出现 EIO；在未修改的 `6d8257d` + fuser 0.18 上用同样路径复现，所以不归因于 0.19，也不把这项报告为通过。保留这一已有示例验收缺口，不扩大依赖更新为 synthetic worker 重构。两次测试挂载均已干净卸载。

未运行 Windows 原生测试或产品端接入；这份记录不证明 crates 已发布。AsterACME 按本次范围暂不更新。
