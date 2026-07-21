# aster_forge_logging

`aster_forge_logging` 提供 Aster 服务共享的 tracing 初始化。它负责日志格式、日志目录、rolling appender 和 guard 返回，不负责产品日志字段和审计事件。

## 适用场景

- 服务启动时初始化 `tracing_subscriber`。
- 控制日志等级。
- 输出文本或 JSON 日志。
- 同时输出到 stdout 和文件。
- 持有 non-blocking appender guard，避免进程退出时丢日志。

不适合放在这里的内容：

- 产品审计日志。
- request id 字段注入。
- 用户行为事件。
- 指标上报。

## Cargo 接入

```toml
[dependencies]
aster_forge_logging = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## 配置

核心类型：

- `LoggingConfig`
- `LoggingInitResult`
- `init_logging(config)`

`LoggingConfig` 可以直接嵌进产品的启动配置结构：

```rust
#[derive(serde::Deserialize)]
struct Config {
    #[serde(default)]
    logging: aster_forge_logging::LoggingConfig,
}
```

典型初始化：

```rust
let logging = aster_forge_logging::init_logging(&aster_forge_logging::LoggingConfig {
    level: "info".to_string(),
    format: "text".to_string(),
    file: "logs/aster.log".to_string(),
    enable_rotation: true,
    max_backups: 5,
});
```

`file` 为空字符串时输出到 stdout。`format` 为 `"json"` 时输出 JSON，其他值使用 text formatter。`RUST_LOG` 会覆盖 `level`，并通过 `LoggingInitResult::warning` 返回提示；`RUST_LOG` 值无效时同样告警并回落到配置的 `level`（"未设置" 与 "无效" 可区分，不会让运维误以为覆盖生效）。全局 subscriber 已存在时（嵌入式运行时、同进程测试）保留现有 subscriber 并告警，不再 panic。

`LoggingInitResult` 持有 guard，产品 entrypoint 必须保存它到进程结束，不能初始化完立刻 drop。

## JSON 日志

JSON 格式适合容器和集中日志系统。产品侧应该决定：

- deployment 默认是 text 还是 json。
- 是否把 request id、user id、task id 写入 span。
- 是否需要按环境覆盖日志等级。

Forge 只初始化 subscriber，不定义业务 span 字段。

## 错误边界

日志初始化失败通常是启动期错误。产品可以选择：

- 文件日志失败则降级 stdout。
- 或者直接启动失败。

无论选哪种，都应该在启动日志里明确记录。

## 测试要求

- text/json 配置能初始化。
- 文件日志启用时 guard 被保存。
- 非法 level 或 format 的产品配置能被拒绝或降级。

## 参考项目

- AsterDrive：容器部署和运维文档里的日志配置。
- AsterYggdrasil：轻量服务启动链路里如何保存 logging guard。
