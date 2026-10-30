# Aster Metrics v1

本合同固定 `aster-metrics/v1`，采用 OpenTelemetry Semantic Conventions 1.44.0。OTel instrument 名、类型、单位和属性是事实源；Prometheus 名称由 exporter 的标准 underscore/单位/Counter 后缀转换产生，OTLP 保留原始 instrument 与 Resource。默认仍为显式启用的 Prometheus pull，可选 OTLP HTTP/protobuf；两条 reader 只记录一次，不能把双路写入同一存储后的样本重复相加。

## 所有权与迁移

| 原实现 | Forge v1 | 产品保留职责 | 必测行为 |
| --- | --- | --- | --- |
| HTTP 私有计数和耗时 | 标准 server duration/active 与有限结果分类 | 访问准入、真实 route、排除探针目录 | 正常、错误、未知路径、取消、body 生命周期 |
| DB 查询私有名称 | 标准 DB operation duration | writer/reader、数据库配置和业务成功边界 | 后端、操作、错误、安全属性 |
| process 私有名称/毫秒 Gauge | 标准 uptime、CPU 秒累计量、物理内存 | 实例生命周期和采样启用 | 首次未知、累计量不重复加、重启 |
| auth/config/task/health 私有指标 | `aster.*` 的共享机制合同 | 业务类型、事务确认和角色适用范围 | 有限标签、取消、失败、就绪事实 |
| Drive 产品指标 | exporter-independent typed instruments | `aster.drive.*`、实际消费看板、共享积压作用域 | schema、双导出、过期、空值、无双发 |

不保留旧序列别名或长期双写；消费者须同时切换对应应用 revision、Grafana/告警与抓取合同。旧 TSDB 历史不改写，跨 schema 趋势不能伪装连续。部署、生产切换不属于库内测试证据。

## 语义与安全

每个实例显式拥有 provider、Resource、instrument catalog、系列预算与关闭流程；不安装全局 provider。标准无覆盖的扩展使用 `aster.*`，Drive 业务使用 `aster.drive.*`。属性来自固定目录，不含用户、文件、URL、SQL、DSN、凭据和自由错误文本。共享数据库快照按相同 store scope 取新鲜有效代表，不能跨副本相加；失败保留 last-good，首次未知不补零。

HTTP Classic 秒桶采用 `[0.005,0.01,0.025,0.05,0.075,0.1,0.25,0.5,0.75,1,2.5,5,7.5,10]`。Native Histogram 尚未做端到端验收，不宣称支持。HTTP 无匹配时省略 `http.route`，未知 method 为 `_OTHER`；duration/active 到 body EOF/error/drop，独立文件流指标是产品数据面观测，不能与请求总量相加。

CPU 模式、pool 和任务语义只能来自真实 API；缺少可证实的数据时登记缺口，不能伪造零、从累计等待量造分位数，或把 handler 返回等同事务确认。

## v1 实现范围

共享标准为 HTTP server duration/active、DB operation duration、SQLx pool count/max、process uptime/CPU/memory。HTTP 扩展仅分类 transport 完成/拒绝/取消/错误，不宣称事务 write-result。role ready 由 ready endpoint 同一结果更新，首次调用前省略；health diagnostics 保留独立 0/1/2 状态。

CPU 是平台合并累计时间，不输出无法从 sysinfo 证明的 user/system 分拆。SQLx 没有公开累计 pool waits 与获取超时，不伪造。任务 handler active/duration 是本地执行事实，确认的状态转换另计；完整 lease/recovery/finish 操作遥测、标准 outbound HTTP/RPC adapter 不在本次已实现清单，不以通用计数器冒充。该范围是 Aster profile，不宣称完整实现 AutoBits RFC 的所有角色组。

属性组合预算由 descriptor 的固定有序 key 与调用方 enum/static catalog 约束；默认每 instrument 4096、实例 16384 个组合，超限拒绝并累计本地失败。identity 固定 service namespace/name/version/instance/environment/role/schema；实例 ID 不进入业务维度。产品 role 目录和业务指标另在 Drive 登记。
