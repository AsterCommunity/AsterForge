# aster_forge_panic

`aster_forge_panic` 提供共享 panic hook 和 crash report writer。它负责在 panic 时输出用户可见提示、写 crash log、捕获 backtrace，并给出 issue report 目标。

## 适用场景

- 服务启动时安装统一 panic hook。
- panic 时写入 `data/crash.log` 或产品指定路径。
- 在 stderr 输出简洁崩溃提示。
- 记录 app name、version、repository、platform、thread、location、message、backtrace。

不适合放在这里的内容：

- 产品级错误恢复。
- 进程 supervisor。
- telemetry 上报。
- 自动创建 GitHub issue。

## Cargo 接入

```toml
[dependencies]
aster_forge_panic = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## 安装 hook

```rust
aster_forge_panic::install_panic_hook(
    aster_forge_panic::PanicHookConfig::new(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_REPOSITORY"),
    )
    .with_crash_log_path("data/crash.log"),
);
```

Rust panic hook 是进程级全局状态。第一次安装的配置会被保留，后续不要在测试或子模块里随便重复安装不同配置。

## 配置

核心类型：

- `PanicHookConfig`
- `DEFAULT_CRASH_LOG_PATH`
- `DEFAULT_ISSUE_TEMPLATE`

可配置项：

- `app_name`
- `version`
- `repository`
- `crash_log_path`
- `issue_template`

## 接入边界

panic hook 是最后防线，不是错误处理机制。产品代码仍然应该正常返回 `Result`，尤其是启动、数据库、外部认证、任务执行这些路径。

如果 panic hook 写 crash log 失败，会把完整 report 输出到 stderr。这是为了确保容器日志里至少有诊断信息。stderr 写出是 best-effort：stderr 关闭或断管时写入错误被忽略——在 panic hook 里 panic 会双重 panic 直接 abort，恰好丢掉这个 crate 要捕获的崩溃诊断。

## 测试要求

- 渲染 crash report 的纯函数路径应覆盖。
- 产品启动测试确认 hook 安装不影响正常日志初始化。
- 不要在普通单元测试里触发真实 panic hook 写全局文件，容易污染并发测试。

## 参考项目

- AsterDrive：可参考启动期安装和 issue template 配置。
- AsterYggdrasil：已删除本地薄封装后，可直接调用 Forge panic hook。
