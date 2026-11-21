# 2026-11 依赖维护与审计修复

本次为尚未发布的依赖维护变更；已有公共时间类型和持久化表示保持不变。

- `cargo upgrade --incompatible allow --pinned allow --recursive false` 和 `cargo update` 均已实际执行；fuser 0.18 → 0.19，Actix Web 4.19 → 4.20，lettre 0.11.23 → 0.11.24，SQLx 0.9.0 → 0.9.1、SQLite 原生库 0.37 → 0.40。
- 依赖版本、features 和来源在根 workspace；成员继承，不在成员中固定第三方版本。模板和 release 工具有自己的独立 workspace。
- 仅 panic 默认时间戳内部使用 `time`；DB、tasks、runtime、API、WebDAV 与服务模板继续使用 chrono，避免在依赖安全更新中改变 SQLite 文本编码及公共 Rust 类型。OIDC 等上游也仍使用 chrono。维护者决定暂时接受 `RUSTSEC-2026-0543` 停维护告警，以精确例外记录到 audit.toml，移除条件关联 #75 及剩余上游依赖；不豁免 chrono 其他安全公告，不表示完成迁移或修复。
- 删除 `.cargo/audit.toml` 中 SQLite 和 crypto-bigint 两条例外。保留四条：RSA 0.9 受 OIDC 版本范围约束；h2 0.3.27 受 Actix HTTP 3.22 约束；paste 仅 Linux allocator 编译期；astral-tokio-tar 仅测试容器。另增加关联 #75 的 chrono 停维护例外，共五条；例外保留残留风险，不代表修复。
- 仅剩的可见落后项是传递约束：crypto-common 0.1.7 精确要求 generic-array 0.14.7，Axum 0.8.9 精确要求 matchit 0.8.4；不强行破坏依赖约束。
- audit workflow 每天 09:17 UTC（北京时间 17:17）执行。

## 时间类型与持久化边界

本次不迁移 chrono 公共 API。`DateTime<Utc>` 参数/返回值、SeaORM 实体、SQLite 文本时间编码、任务去重 key、租约与调度时间运算均保持已有形式；产品不需要替换 DTO 或 trait 时间类型。完整 chrono 迁移由 [Issue #75](https://github.com/AsterCommunity/AsterForge/issues/75) 单独跟踪，需验证旧数据、混合精度排序、范围查询、cursor 等值匹配以及各消费端，不能仅凭新库 round-trip 通过决定兼容。

panic crate 的时间函数没有暴露 chrono 类型；其内部改用 time，同时保留 UTC 毫秒格式和不可表示时钟的失败处理。没有新增 jiff 直接依赖。`missing_docs` lint 留待单独文档任务，不在本轮启用；unsafe lint 保持原配置。

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

## 回归与供应链核验

回退后 workspace 测试为 1733 passed、0 failed、7 ignored；DB 业务 features 为 141 passed，tasks/runtime 功能组合为 249 passed，API OpenAPI 为 32 passed，audit DB writer/lifecycle 为 23 passed。Rust 1.95 workspace all-targets check、stable workspace 严格 Clippy、格式与 release 元数据检查、文档站/rustdoc 构建均通过。配置 chrono 例外前，audit 为 0 个未豁免漏洞、1 个 chrono 停维护告警；本轮按维护者决定增加精确停维护例外，共五条审计例外，不能将告警被接受描述为修复。SMTP 六类依赖边界回归保留，包括未终止的超长回复、STARTTLS 后的额外明文、quoted local part、显示名换行、空白 EHLO 以及裸 CR/LF/分块 dot-stuffing。SQLx 0.9.1 下 rollback 失败通过具名事务 fixture 注入，不依赖已被驱动正确识别的裸 SQL ROLLBACK。

fuser 0.19.0 的 crates.io Trusted Publishing 指向 `cberner/fuser`，run `44795244825`、tag 和包内 VCS SHA 均为 `893dfeb4854828ad7ea1d68c934fa6a6964061f7`。缓存 .crate SHA-256 为 `94a147f6d231dc1eddfe6e2ad35d6c3676a5bd6776d3975c8703ea4679f90cc3`，与 registry 一致；86 个非生成文件与上游 tag 提交逐字节相同。0.18→0.19 无新增生产依赖；build.rs 只删去 macfuse-4-compat cfg 输出，挂载/卸载、Owner/killpriv、statx 和参数校验是主要改动。源码 unsafe 关键词 30→34，位于既有系统调用/FFI 边界；这不是 Socket 72 分的精确归因。Socket 在该 PR head 的检查显示 No new dependency alerts，分数也不能独立证明漏洞或恶意包。

Linux cloud-files/panic 的已有 native tests 与真实 FUSE 挂载证据不涉及本次回退的 chrono API。挂载验证曾覆盖读写、truncate、fsync/reopen、独占创建、并发定位写入、mkdir、跨目录 rename、稳定 inode、NOREPLACE 与错误映射；覆盖已有目标的 rename EIO 在未修改的 `6d8257d` + fuser 0.18 同样复现，因此保留为原有示例验收缺口。测试挂载均已卸载。

未运行 Windows 原生测试或产品端接入；版本递增不证明 crates 已发布。AsterACME 按本次范围暂不更新。

## 本轮 review 收尾

PR #73 review `6003753407` 的 8 项中，SQLite 排序/旧数据等值匹配、迁移测试容器边界、time 转 UTC 越界、retry 日期范围、WebDAV 占位日期、迁移文档 Option default 共 7 项随公共 chrono 迁移撤出，并保留到 #75 的验收要求。代码对照迁移前 `6d8257d` 逐字节复核；不把新记录测试通过当旧数据兼容。

仍适用的 STARTTLS 测试问题已用脚本化 `AsyncTokioStream` 修复，通过 `connect_with_transport` 注入两行同批读取，不依赖 TCP 时序或 sleep。`handshake` 改为借用 listener，修复 `needless_pass_by_value`。Service template CI 曾因迁移后 build.rs 格式失败；模板源只随 chrono 撤回恢复原状。维护者明确暂不处理 `Rust CI / Service template / Generate and verify Actix service template` 的远程失败；本地生成工程检查不等同于该 CI 已转绿。

`unused workspace dependency aster_forge_process` 对应未被任何成员继承的根声明，已移除声明但保留 workspace 成员。删除 cache dashmap、audit/observability tracing 真正无用依赖；alloc tracing 限于 jemalloc-stats，middleware tower 留在测试，测试容器 JSON 经 Forge HTTP feature 启用。未开启 prometheus 的 observability 默认构建仍可能提示 actix-web 未用：实际 handler 是 actix+prometheus 的交集，完整 exporter 测试通过，不删除有效 adapter 依赖或用虚假引用隐藏提示。
