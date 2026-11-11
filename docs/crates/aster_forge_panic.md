# aster_forge_panic

提供显式安装的进程级 panic hook、backtrace 捕获与 best-effort 报告。完整开发报告与公开诊断分离；数据目录、JSON envelope、隐私、恢复和退出策略归产品所有。没有 runtime/observability 依赖，也不会发送网络请求。

## 适用场景与边界

仅在可执行程序入口、创建工作线程前安装。嵌入式库不能接管宿主 hook。正常错误继续返回 `Result`；`JoinError`、`catch_unwind` 和退出策略由产品处理，hook 不是恢复机制。

## Cargo 接入

```toml
[dependencies]
aster_forge_panic = { git = "https://github.com/AsterCommunity/AsterForge" }
```

没有 feature flag。Linux 文件策略依赖 rustix，时间使用 chrono；JSON 序列化由产品选择。

## 最小接入

```rust
use aster_forge_panic::{install_panic_hook, PanicHookConfig, PanicOutput};

let outcome = install_panic_hook(
    PanicHookConfig::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), "")
        .with_crash_log_path("data/crash.log")
        .with_output(PanicOutput::Silent),
);
```

默认 `PanicOutput::Human` 在 stderr 输出简短提示；`Silent` 完全不输出；`Sink(Arc<dyn DiagnosticSink>)` 由产品选择输出通道和 JSON envelope。见[可编译 CLI 示例](https://github.com/AsterCommunity/AsterForge/blob/master/crates/aster_forge_panic/examples/cli.rs)。

`PublicDiagnostic` 只有应用名、时间、报告写入结果，永远没有原始 payload、location、backtrace、文件路径或 writer 错误。失败也不会将完整报告回退到 stderr。应用名和自定义时间是产品提供的公开值，不能放凭据。

## 时间、私有 writer 与隐私

默认时间固定为 UTC RFC3339 毫秒（`Z`），可通过 `.with_timestamp(fn() -> String)` 注入产品策略或固定测试时间。

完整报告含 payload、源码路径和 backtrace，**可能含敏感信息**。仅 `ReportWriter` 接收报告；通过 `.with_report_writer(Arc<dyn ReportWriter>)` 接入产品私有文件、过滤/脱敏和持久化策略。writer 返回的错误不会进入公开输出。

默认 `PrivateReportWriter` 在安装时打开文件，不在 panic 时延迟创建：

- Linux 逐级使用目录描述符与 NOFOLLOW，拒绝路径中的符号链接、`..`、特殊文件与硬链接；新目录 0700，新文件 0600。
- 已有文件必须属于当前有效 UID，且无 group/other 权限；拒绝不安全文件，既不 chmod 也不截断。允许私有已有文件追加。
- 产品/OS 必须管理祖先目录；不防御相同 UID 的恶意进程。含 OS 符号链接的可信根应由产品启动阶段先解析可信根，然后追加托管路径。
- 其他平台（包括 macOS）默认拒绝落盘，产品应注入执行私有 ACL 策略的 writer。macOS ACL 可在 0600 模式位之外授权其他用户，当前默认 writer 不具备可靠 ACL 校验，不能以模式位冒充完整隐私保证。
- `PrivateReportWriter::try_new` 可在正常启动阶段返回具体错误；默认打开失败仅使报告不可用，不阻止 hook 安装。

## 安装与失败合同

`install_panic_hook` 返回 `InstallOutcome`。首次安装替换已有 hook，**不链式调用**旧 hook，避免旧输出泄漏完整报告。后续调用返回 `AlreadyInstalled` 且不执行 `set_hook`；宿主后来替换 hook 时也不重新抢回。panicking thread 调用返回 `PanickingThread`。并发宿主自行调用 `set_hook` 不在此合同内。

hook 使用 try_lock 串行化完整处理；其他线程同时 panic 时跳过报告，不等待占用的 hook。默认文件 writer 也使用 try_lock。文件与 stderr I/O、用户 writer/sink 错误均 best-effort 忽略，不产生原始报告回退。同步文件/输出 I/O 本身没有超时保证。

**自定义 writer、sink 和时钟必须不 panic、不阻塞等待锁。** Rust 在 hook 内二次 panic 会直接 abort，不能依赖 `catch_unwind` 救回。分配失败、栈溢出、abort 类故障也不保证报告生成。

## 测试要求

纯函数覆盖非字符串 payload、缺失 location、UTC；真实全局 hook 使用独立子进程验证 human/JSON/silent、首次安装、旧宿主 hook 交互、writer/sink 失败与并发 panic。文件测试覆盖追加、权限、链接、目录失败与锁占用。不要污染测试进程全局状态或仓库 `data/`。

## 参考项目

AsterDrive / AsterYggdrasil 提供服务入口安装参考；AsterACME 的 CLI 输出与私有状态策略由其产品 adapter 实现，本票不自动更新下游依赖。
