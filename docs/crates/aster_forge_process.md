# aster_forge_process

`aster_forge_process` 提供 Aster 服务可复用的子进程生命周期机制。它只负责执行与资源边界，不解释产品协议，也不决定失败后能否重试。

## 所有权边界

本 crate 负责：

- 直接执行程序与参数，不经过 shell。
- 可选 stdin 输入、显式环境变量及可清空的继承环境。
- 并发排空 stdout/stderr；分别限制保留字节数，超出部分继续读取但不保存。
- 取消、相对启动期限、退出观察，以及拥有的后代进程清理。
- Unix process group 与 Windows Job Object 生命周期。
- `ProcessSpec` 调试输出隐藏 stdin 和环境变量。

调用方负责协议校验、期限来源、输入/输出敏感级别、错误映射、日志/展示、重试与副作用未知状态。`on_chunk` 只收到保留的输出片段，会在 Tokio 任务中同步调用，应快速返回且不得阻塞。

## API

```rust
use aster_forge_process::{ProcessSpec, run};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

let spec = ProcessSpec::new(program, Duration::from_secs(30));
let result = run(spec, &CancellationToken::new(), |_stream, _bytes| {}).await;
```

`ProcessSpec::new` 默认继承环境，并为每路输出保留最多 64 KiB。`clear_environment` 可清空继承环境，之后显式加入 `environment` 项。零期限和启动前取消不会启动进程。启动后取消或超时会终止所拥有的进程树；丢弃运行中的 future 也会触发清理。

`ProcessOutcome` 区分直接 child 是否启动、退出状态、两路输出截断/读取错误、stdin 写入错误和 `cleanup_error`。直接 child 的退出先单独观察；随后终止拥有的 process group/job，并等 Unix group 或 Windows descendant tree 为空。Unix 清理等待有界；宿主无法回收的 adopted zombie 会通过 `cleanup_error` 报告，而不是让调用永久挂起。清理本身失败时写入 `cleanup_error`，调用方必须将其视为执行失败，不得据直接 child 的成功状态释放仍受保护的资源。Unix 子进程若主动脱离所属 group，调用方不得假设本 crate 能终止该 daemon。

```toml
[dependencies]
aster_forge_process = "=0.1.0-alpha.3"
tokio-util = { version = "0.7", features = ["rt"] }
```

## 验证

`cargo test -p aster_forge_process --locked --all-targets` 使用真实子进程覆盖双向管道、精确输出上限、过量输出排空、stdin 断开、环境清理、敏感调试输出、缺失可执行文件、启动前取消/零期限、取消/期限后的后代终止（并断言 `run` 返回时后代已退出）、正常退出清理后代和 future 丢弃清理。

Linux、macOS、Windows CI 都在原生 runner 执行这些边界测试；它们验证模拟子进程与宿主 OS 进程所有权机制。Unix 子进程若主动脱离所属 process group，调用方不得假设本 crate 能终止该 daemon。

`cargo +nightly miri test --locked -p aster_forge_process --lib` 实际执行内存管道、启动前取消/零期限和非原生 wrapper 的 `unsafe` 遍历测试；CI 在通过测试数为零时失败。Miri 不执行真实子进程、Unix 进程控制或 Windows Job Object FFI，不能替代三平台原生契约测试。
