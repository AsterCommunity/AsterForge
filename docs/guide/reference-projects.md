# 参考项目

Forge 文档会频繁引用 AsterDrive 和 AsterYggdrasil，因为这两个项目是当前共享 crate 的主要来源和首批接入方。

## AsterDrive

仓库链接：[AsterCommunity/AsterDrive](https://github.com/AsterCommunity/AsterDrive)

适合作为参考的场景：

- 文件与文件夹 API 的分页、排序和 cursor。
- 缓存、数据库连接、运行时任务和指标记录。
- 外部认证连接器和专用 provider 配置。
- 存储 key、S3 兼容配置和对象存储边界。
- 文件类型分类、上传、分享、WebDAV、WOPI 等复杂业务周边。

Drive 的特点是业务面更大，很多 Forge crate 的完整用法会先在 Drive 里出现。但也因为它更复杂，不能把 Drive 的业务层抽象直接搬进 Forge。

## AsterYggdrasil

仓库链接：[AsterCommunity/AsterYggdrasil](https://github.com/AsterCommunity/AsterYggdrasil)

适合作为参考的场景：

- Actix middleware 接入。
- 外部认证的较轻集成。
- 后台任务系统接入 Forge lease、dispatch、runtime primitives。
- shared panic hook、logging、metrics、DB handles 的服务启动/关闭链路。
- OpenAPI feature 下的 API schema 和 route annotation。

Yggdrasil 的特点是接入边界更清楚，适合看“产品侧应该保留什么”。例如后台任务里，task kind、payload/result、SeaORM repository 留在 Yggdrasil，lease、heartbeat、dispatch lifecycle 放到 Forge。

## 观测消费者证据索引

### Aster Metrics v1 候选（2026-10-31）

[Forge #53](https://github.com/AsterCommunity/AsterForge/pull/53) 提供 [v1 profile](./metrics-profile.md)，实现 revision 为 `9797a21eae307194e75443ad88ffecf2c4b61b41`。[Drive #662](https://github.com/AsterCommunity/AsterDrive/pull/662) 的 `3a3aa00b26c39c57a15a82e2c391663d3a99a4a0` 固定该 Git revision，无路径 patch；迁移 49 个产品 instrument、生成目录和 Grafana。下方日期化表格保留旧版证据，不作为 v1 schema 兼容承诺。

本地验证包含实际 Prometheus/OTLP wire、Noop/reader 组合、middleware body 生命周期、原生 SQLite pool、Drive profile/Grafana、定向健康/认证/采样/任务测试、Clippy、工作区检查与文档构建。PR、库验证、消费者接入和生产部署分别验收；本次没有发行镜像、真实 Collector 持久化或生产 rollout 证据。其他固定旧 revision 的消费者未自动升级，需同步其指标消费端。

核对日期：2026-10-17。本索引由 Forge 维护公共机制、固定版本和消费者证据引用；Drive 拥有[当前能力与配置事实源](https://github.com/AsterCommunity/AsterDrive/blob/master/docs/src/content/docs/ops/monitoring.md)。`master` 链接表示当前分支；下表固定提交只证明该快照，不推断发行或生产状态。这里不复制 Drive 的完整支持矩阵。

| 字段 | AsterDrive 证据 |
| --- | --- |
| capability / owning crate | `aster_forge_metrics`：实例 provider、typed instruments、Prometheus / OTLP HTTP/protobuf metrics 与 lifecycle；`aster_forge_observability`：Actix exposition；`aster_forge_logging`：本地 text/JSON subscriber 与 guard |
| Forge revision | [`869756f0605c898420a28d3d45006cc33a87d17c`](https://github.com/AsterCommunity/AsterForge/commit/869756f0605c898420a28d3d45006cc33a87d17c)；[metrics 合同](https://github.com/AsterCommunity/AsterForge/blob/869756f0605c898420a28d3d45006cc33a87d17c/docs/crates/aster_forge_metrics.md)、[observability 合同](https://github.com/AsterCommunity/AsterForge/blob/869756f0605c898420a28d3d45006cc33a87d17c/docs/crates/aster_forge_observability.md)、[logging 合同](https://github.com/AsterCommunity/AsterForge/blob/869756f0605c898420a28d3d45006cc33a87d17c/docs/crates/aster_forge_logging.md) |
| consumer / revision / lock source | Drive [`4b857c0fc3a27941b36861657225f20786c3831b`](https://github.com/AsterCommunity/AsterDrive/commit/4b857c0fc3a27941b36861657225f20786c3831b)；[Cargo.lock](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/Cargo.lock)、[根 features](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/Cargo.toml)、[产品 crate features](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/crates/aster_drive_metrics/Cargo.toml)；固定 Git revision，无本地 patch |
| integration status | 实例指标、多导出与抓取认证已由 [Drive #642](https://github.com/AsterCommunity/AsterDrive/pull/642) 合并；本地 logging 已接入；OTLP traces/logs、跨节点 trace 和 continuous profiling 平台导出未接入 |
| adapter / call sites | [Drive recorder](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/crates/aster_drive_metrics/src/lib.rs)、[静态配置](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/src/config/metrics.rs)、[health route](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/src/api/routes/health.rs)、[认证](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/src/api/middleware/metrics_auth.rs)、[runtime 注入](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/src/runtime/components.rs)、[shutdown](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/src/runtime/assembly.rs)、[logging 初始化](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/src/main.rs) |
| validation evidence | #642 记录本地四 feature/config 组合、primary/follower 端点认证、schema/Grafana、上传和三库 migration；当时 Forge 为 `2beaf986f1740629d3fd8913f865e4134d31e203`，未等待新 CI。当前固定 revision 未据此宣称重跑全部矩阵；[可重复命令](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/scripts/test-metrics-features.sh) |
| rollout evidence | [镜像 workflow](https://github.com/AsterCommunity/AsterDrive/blob/4b857c0fc3a27941b36861657225f20786c3831b/.github/workflows/docker-image.yml) 定义 `-metrics`，不证明具体镜像 tag/digest 已发行；本次未核验可下载产物或生产部署。OTLP 按 Drive 的源码/自建镜像说明启用 |
| next issue | [Forge #44](https://github.com/AsterCommunity/AsterForge/issues/44) 收尾库级/模板验收；[Forge #45](https://github.com/AsterCommunity/AsterForge/issues/45) → [Drive #638](https://github.com/AsterCommunity/AsterDrive/issues/638) 交付 traces；[Forge #46](https://github.com/AsterCommunity/AsterForge/issues/46) ↔ [Drive #639](https://github.com/AsterCommunity/AsterDrive/issues/639) 维护事实源与证据 |

### 更新与验证责任

- 公共 API、feature、导出协议、schema 或生命周期变化时，同一 PR 更新 crate 文档、受测示例和本索引；影响消费者时链接其固定 revision/PR 与实际测试结果。产品配置与角色矩阵留在消费者事实源。
- 合并、依赖锁定、库内测试、消费者测试、发行产物和生产 rollout 是不同证据。未知写“未验证”；本地 patch/WIP 必须明确标记，不能作为已发布 Git 接入。
- `bun run docs:build` 在 `docs/` 执行，先执行 `docs:check-evidence` 验证索引必填字段、中英固定 revision 和 Forge 源码路径，再验证站内链接并构建 rustdoc；消费者测试复用其脚本，不要求 Forge 每个 PR 拉取所有产品。Drive 文档的离线版本检查约束中英基线与 lock；静态检查不证明协议行为或 collector 送达。
- #44 的核心内核已实现，剩余验收不等于 Drive 未迁移。#45 的 tracing/layer 是未来合同；当前 `init_logging()` 安装全局 subscriber，不可把已有日志 span 写成已支持 OTel 导出。
