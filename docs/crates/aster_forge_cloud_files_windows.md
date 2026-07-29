# aster_forge_cloud_files_windows

`aster_forge_cloud_files_windows` 是 Windows Cloud Files API（CFAPI）的产品无关绑定层。它把 `aster_forge_cloud_files_core` 的稳定 `CloudItemKey` 映射成 Windows `FileIdentity`，验证 CFAPI 的 native 限制，并将 owned placeholder request 转换为 `CF_PLACEHOLDER_CREATE_INFO` 后调用 `CfCreatePlaceholders`。它也提供 owned sync-root registration model，将稳定 provider/scope identity 和经过平台版本验证的 policy 转换为 `CF_SYNC_REGISTRATION`、`CF_SYNC_POLICIES` 并调用 `CfRegisterSyncRoot`。

当前 crate 已完成 Windows Phase 2 PoC：identity/placeholder、persistent sync-root registration、active callback connection/session lifecycle、hydration/restart、active waiter cancellation、watchdog/progress、validate/dehydrate/delete/rename acknowledgement、completion observation 和 native eviction/pin/in-sync effect。Windows target 已直接调用 `CfConnectSyncRoot` / `CfDisconnectSyncRoot`，注册 `FETCH_DATA` 与 `CANCEL_FETCH_DATA` callback，复制 owned snapshot 并通过调用方提供的 bounded `SyncSender` 执行非阻塞 ingress。队列拒绝、closing 和未被 worker 显式终结的 fetch request 都有结构化失败完成路径；worker 也可以把 stable item identity 对应的 product-owned `ContentRevision` 交给 core `HydrationCoordinator`，并以 owned bytes 调用成功 `TRANSFER_DATA`。`CANCEL_FETCH_DATA` 会先直接匹配 registry 中的 core waiter，再把 owned snapshot 作为可选观察事件送入 bounded queue；host-polled 60 秒 watchdog、monotonic progress reporting 和 ingress queue metrics 已完成。`FETCH_DATA_FLAG_RECOVERY` 会显式暴露给产品 adapter，revision 失效时可用 `RESTART_HYDRATION` 更新 metadata/identity 并让 CFAPI 重发 hydration callback。

## 所有权边界

这个 crate 负责：

- `CloudItemKey <-> FileIdentity` 的版本化编码与解码；
- 编码完成后的 CFAPI 4 KiB identity limit；
- Windows 单路径组件名称验证；
- Windows file time、file attributes 和 signed file-size 映射；
- owned placeholder request；
- Windows target 上的 `CfCreatePlaceholders` 调用与 per-entry result/USN 提取；
- `CloudScope <-> SyncRootIdentity` 的版本化编码与 64 KiB boundary；
- provider name/version/GUID、root file identity 和 registration flags；
- hydration、population、in-sync、hard-link 与 placeholder-management policies；
- CFAPI platform integration gate 和 Windows target 上的 `CfRegisterSyncRoot` 调用；
- `CfConnectSyncRoot` / `CfDisconnectSyncRoot` active connection；
- callback context、panic isolation、owned callback snapshot 和 bounded ingress；
- `Accepting -> Closing -> Draining -> Closed` lifecycle 与 `SessionGeneration` fence；
- `FETCH_DATA -> HydrationCoordinator -> TRANSFER_DATA` 成功闭环；
- generation/connection/transfer/request/file correlation 的 active waiter registry；
- full-cover cancellation 到 core waiter cancellation，以及 partial-overlap 保留；
- active waiter 与 cancellation outcome process-local metrics；
- queue rejection、closing、drop、hydration error 和显式 fetch failure 的 terminal `CfExecute`；
- `VALIDATE_DATA`、dehydrate/delete/rename preflight ACK 与 completed observation；
- unregister、pin、in-sync 与 provider-triggered dehydrate native physical effect。

这个 crate 不负责：

- 远端 API、认证、token、endpoint 或 DTO；
- 个人盘、团队盘、组织、权限、配额和冲突展示；
- 产品数据库 entity、历史 migration 或 repository query；
- 决定 `CloudItemKey`、revision、hydration、mutation、upload 或 eviction 的产品无关语义；
- executable 安装、sync-root 产品注册策略和升级流程。

`aster_forge_cloud_files_core` 仍然拥有操作系统无关机制，产品 crate 只向 Windows adapter 提供 backend/store 实现和产品边界映射。

## Cargo 集成

```toml
[dependencies]
aster_forge_cloud_files_core = { path = "../AsterForge/crates/aster_forge_cloud_files/core" }
aster_forge_cloud_files_windows = { path = "../AsterForge/crates/aster_forge_cloud_files/windows" }
```

当前没有 crate feature。`windows` 依赖只在 `cfg(windows)` target 启用，portable identity、placeholder model 和 contract tests 可以在 macOS/Linux 开发机编译运行；真正的 CFAPI 函数只在 Windows target 导出。

## FileIdentity 合同

Windows `FileIdentity` 是 adapter mapping，不是 core identity，也不是路径。当前版本化 envelope 编码以下三个 UTF-8 opaque value：

```text
CloudNamespaceId
CloudRootId
CloudItemId
```

格式使用 magic、version、三个 little-endian `u32` byte length 和原始字段 bytes。名称、parent、metadata revision、content revision、digest、账号 token 和产品字段不进入 identity。

因此：

- rename 和同 root move 不改变 identity；
- 相同 item ID 位于不同 namespace/root 时不会碰撞；
- decoder 拒绝 wrong magic、unsupported version、truncated envelope、length mismatch、trailing bytes、invalid UTF-8 和 core empty identity；
- `WindowsFileIdentity::Debug` 只显示格式版本与 byte length，不输出 opaque identity 内容；
- identity 在完整编码后检查 `CFAPI_FILE_IDENTITY_MAX_BYTES == 4096`。

当前格式是 Windows adapter 的 Phase 2 versioned PoC format。后续如果持久 store PoC 证明需要短 indirection key，会通过新的 format version 演进，而不是静默改变 version 1 的解释。

## 最小 identity 示例

```rust
use aster_forge_cloud_files_core::{
    CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};
use aster_forge_cloud_files_windows::WindowsFileIdentity;

let key = CloudItemKey::new(
    CloudScope::new(
        CloudNamespaceId::new("backend-space")?,
        CloudRootId::new("root-id")?,
    ),
    CloudItemId::new("stable-item-id")?,
);

let identity = WindowsFileIdentity::encode(&key)?;
assert_eq!(identity.decode()?, key);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Placeholder 创建模型

`WindowsPlaceholder::from_item` 接收：

- revision-bearing `CloudItem`；
- 已编码的 `WindowsFileIdentity`；
- `WindowsFileTimes`；
- `WindowsPlaceholderOptions`。

constructor 会验证 identity 解码结果与 item key 完全一致。file size 从 `CloudContentMetadata::size` 映射到 CFAPI signed `i64`；directory 使用 size 0 和 `FILE_ATTRIBUTE_DIRECTORY`，regular file 使用 `FILE_ATTRIBUTE_NORMAL`。

placeholder name 当前必须是一个 Windows path component。以下输入会被拒绝：

- `.`、`..`；
- `/`、`\\` 和 Windows reserved characters；
- ASCII control characters；
- trailing space 或 trailing period；
- `CON`、`PRN`、`AUX`、`NUL`、`COM1..COM9`、`LPT1..LPT9`，包括带 extension 的形式。

名称映射策略仍由调用方拥有。如果远端名称不满足 Windows naming contract，产品 adapter 需要在进入 placeholder constructor 前执行自己的稳定映射，并把映射关系持久化；Forge Windows crate 不会把 display name 偷偷变成 identity。

## Windows native 调用

Windows target 导出：

```rust
use std::path::Path;

use aster_forge_cloud_files_windows::create_placeholders;

let outcome = create_placeholders(Path::new(r"C:\sync-root"), &placeholders)?;
for entry in outcome.entries {
    println!("HRESULT={} USN={}", entry.result_code, entry.create_usn);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

native conversion 在调用栈内创建 null-terminated UTF-16 names 和 `CF_PLACEHOLDER_CREATE_INFO` 数组。所有 `PCWSTR`/identity pointer 只引用本函数持有的稳定 buffer，并在同步 `CfCreatePlaceholders` 返回后立即失效；public model 不保存 native pointer 或 borrowed callback object。

当前 wrapper 使用 `CF_CREATE_FLAG_NONE`，entry options 支持：

- mark in sync；
- disable on-demand population；
- always full；
- supersede。

## Sync-root identity 与持久注册

Forge Windows registration 强制携带 `WindowsSyncRootIdentity`，即使原生 CFAPI 把 `SyncRootIdentity` 定义为 optional。原因很简单：Forge 的 restart/callback reconciliation 需要把持久 native registration 精确关联回一个 product-neutral `CloudScope`，不能靠 display name、路径或产品账号 DTO 猜测。

当前 sync-root identity version 1 编码：

```text
magic:          AFSR
format version: 1
namespace len:  little-endian u32
root len:       little-endian u32
namespace bytes
root bytes
```

完整编码后检查：

```text
CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES == 65536
```

root 必须提供一个 `WindowsFileIdentity`，constructor 会解码并验证它与 sync-root identity 属于同一 namespace/root scope；不同 scope 的 root file identity 会被拒绝。该约束保证 root callback 与普通 item callback 使用同一套非空 identity 解码合同，不会出现注册成功但 root callback 无法表示的状态。

`WindowsSyncRootRegistration` 还要求：

- non-empty provider name；
- non-empty provider version；
- provider name/version 各不超过 255 UTF-16 code units；
- provider text 不含 NUL；
- stable non-zero `WindowsProviderId`；
- explicit `WindowsSyncRootPolicies`；
- explicit `WindowsSyncRootRegistrationOptions`。

provider GUID 由产品/发行方选定并跨版本、跨 root 保持稳定。Forge 不采用 CFAPI 的“根据 ProviderName 生成 GUID”隐式行为，因为 display name 改动不应改变 provider telemetry identity。

## Registration policy

hydration primary 只暴露当前真实支持的：

```text
Progressive
Full
AlwaysFull
```

CFAPI 文档明确标记尚未获得平台支持的 `PARTIAL` 没有进入 public enum。population 同样只暴露：

```text
Full
AlwaysFull
```

hydration modifiers 包含：

- validation required；
- streaming allowed；
- auto dehydration allowed；
- allow full restart hydration。

`validation required` 与 `streaming allowed` 是原生冲突组合，portable validation 会在 FFI 前拒绝。`allow full restart hydration` 要求 platform integration `>= 0x500`。

placeholder management 支持 create/convert/update unrestricted 三个独立 permission。任何一个被启用都要求 platform integration `>= 0x310`。

`WindowsInSyncPolicy` 以可组合 bitset 暴露 file/directory creation time、read-only、hidden、system、last-write time，以及 preserve-for-sync-engine。hard-link policy 需要显式选择 `Forbidden` 或 `Allowed`，不会根据产品行为猜测。

registration options 支持：

- update existing registration；
- disable on-demand population on root；
- mark root in-sync during registration。

## `CfRegisterSyncRoot` 调用

Windows target 导出：

```rust
use std::path::Path;

use aster_forge_cloud_files_windows::register_sync_root;

let platform = register_sync_root(Path::new(r"C:\sync-root"), &registration)?;
println!("CFAPI integration={:#x}", platform.integration);
# Ok::<(), Box<dyn std::error::Error>>(())
```

wrapper 会先调用 `CfGetPlatformInfo`，再用检测到的 integration number 验证 gated policy，之后构造准确 `StructSize` 的 `CF_SYNC_REGISTRATION` 和 `CF_SYNC_POLICIES`。provider/sync-root/file identity buffer 与 UTF-16 strings 全部保持 owned，所有 native pointer 只在同步注册调用期间存在。

持久注册与活动 connection 是两个生命周期：`CfRegisterSyncRoot` 成功只表示平台记住了 root、provider identity 和 policies，不表示 callback session 已经建立。后续 `CfConnectSyncRoot` 会使用独立 connection/session generation 和 closing fence。

## Active connection 与 callback ingress

Windows target 的 `connect_sync_root` 接收：

- registered sync-root path；
- non-zero core `SessionGeneration`；
- bounded `std::sync::mpsc::SyncSender<WindowsCallbackRequest>`；
- `WindowsSyncRootConnectOptions`。

connect options 对应 CFAPI 的 process info、full normalized path 和 block-self implicit hydration flags。默认值不请求任何 optional behavior，不根据产品进程、账号或安装方式猜测。

```rust
use std::{path::Path, sync::mpsc::sync_channel};

use aster_forge_cloud_files_core::SessionGeneration;
use aster_forge_cloud_files_windows::{
    WindowsCallbackRequest, WindowsFetchDataFailure, WindowsSyncRootConnectOptions,
    connect_sync_root,
};

let (sender, receiver) = sync_channel::<WindowsCallbackRequest>(128);
let generation = SessionGeneration::new(1)?;
let mut connection = connect_sync_root(
    Path::new(r"C:\sync-root"),
    generation,
    sender,
    WindowsSyncRootConnectOptions {
        require_full_file_path: true,
        ..WindowsSyncRootConnectOptions::default()
    },
)?;
let fetch_waiters = connection.fetch_data_waiters();

// 产品 adapter 继续拥有 metadata/revision lookup；Forge 不从 path 猜 revision。
if let Ok(WindowsCallbackRequest::FetchData(request)) = receiver.recv() {
    let item_key = request.snapshot().info().file_identity().decode()?;
    let revision = product_metadata_store.current_content_revision(&item_key).await?;
    request
        .hydrate(
            &hydration_coordinator,
            revision,
            backend_alignment,
            &fetch_waiters,
        )
        .await?;
}

connection.disconnect()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

native callback table 当前注册：

```text
CF_CALLBACK_TYPE_FETCH_DATA
CF_CALLBACK_TYPE_CANCEL_FETCH_DATA
CF_CALLBACK_TYPE_VALIDATE_DATA
CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE
CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE_COMPLETION
CF_CALLBACK_TYPE_NOTIFY_DELETE
CF_CALLBACK_TYPE_NOTIFY_DELETE_COMPLETION
CF_CALLBACK_TYPE_NOTIFY_RENAME
CF_CALLBACK_TYPE_NOTIFY_RENAME_COMPLETION
CF_CALLBACK_TYPE_NONE sentinel
```

### Owned snapshot

callback trampoline 会在返回前复制：

- native connection/transfer/request key scalar；
- volume names、normalized path 和 optional process info wide strings；
- sync-root identity 与 file identity bytes；
- file ID、file size、priority hint；
- required/optional fetch range、`CF_EOF`、dehydration data 和 flags；
- cancellation range 与 timeout/abort flags。

`WindowsCallbackInfoSnapshot` 会重新验证 identity envelope，并确认 file identity 与 sync-root identity 属于同一 `CloudScope`。native pointer、`PCWSTR`、`CF_CALLBACK_INFO` 和 `CF_CALLBACK_PARAMETERS` 都不会进入 channel、worker 或 async boundary。Debug 输出不包含 opaque identity、path 或 process command line。

raw callback 只在窄 FFI helper 中存在。helper 先单独复制 `StructSize` / `ParamSize`，确认完整 `CF_CALLBACK_INFO`、`CF_PROCESS_INFO` 或当前 union member 的真实 ABI offset/size 已包含在 native buffer 中，再执行一次对应结构复制；不会先构造完整 callback union reference。callback context 也在 trampoline 内立即克隆为 owned session/sender handle，不向 handler 下游传播裸 context pointer。identity length 在构造 slice 前按 4 KiB / 64 KiB 上限验证，callback wide string 使用 32767 UTF-16 unit 的有界扫描。每个 `unsafe` block 只包围一次必要的 pointer read、slice construction 或 CFAPI call。

### Generation、closing 与 drain

core `SessionGeneration` 是 durable fence，CFAPI `ConnectionKey` 是当前 native channel 的 opaque key，两者由不同类型承载。callback request 必须同时持有：

- generation-bearing owned snapshot；
- 同一 session 发出的 non-cloneable `WindowsCallbackLease`。

受控 constructor 会拒绝 snapshot/lease generation mismatch。调用 disconnect 时先建立 closing fence，因此 `CfDisconnectSyncRoot` 调用期间仍到达的 callback 会被明确失败，不会进入 worker。native disconnect 返回后不再接收 callback，Box-backed context 才会释放；已经进入 channel 的 owned request 不借用 context，并通过 lease 继续 drain。最后一个 lease 释放后 session 才进入 `Closed`。

显式 `disconnect` 和 `Drop` 共享连接状态机。native disconnect 失败后状态保持可重试，后续显式调用会再次执行 `CfDisconnectSyncRoot`；同时 bounded ingress sender 会独立关闭，让 worker receiver 正常结束。只有对象最终 Drop 且重试仍失败时，才保留 CFAPI 仍可能引用的稳定 callback context，避免释放后继续收到 callback 造成悬空访问；被保留的 context 已不再持有 sender。

### Hydration 与 terminal completion

CFAPI callback 不携带 core `ContentRevision`。产品 worker 必须先用 owned snapshot 中的稳定 `CloudItemKey` 查询产品 metadata/store，再调用：

```text
WindowsFetchDataRequest::hydrate(
    &HydrationCoordinator,
    ContentRevision,
    backend_alignment,
    &WindowsFetchDataWaiterRegistry,
)
```

Windows adapter 会把 callback item key、file size、required range、session generation、CFAPI 4096-byte alignment 和 backend alignment 组合成 `HydrationRequest`。`CF_EOF` 会解析为直到 snapshot file size 的精确 range；offset 必须 4096-byte aligned，非 EOF transfer length 也必须 4096-byte aligned。coordinator 返回的 revision、offset、length 和 total size 必须完全匹配，随后其 owned `Bytes` 在同步 `CfExecute` 期间保持存活，不进行额外 payload copy。

高级 worker 可以调用 `prepare_transfer` 自己编排不带 native cancellation 的读取，也可以调用 `prepare_registered_transfer` 获得 `Transfer` / `Cancelled` / `TimedOut` portable outcome。前者只等待 coordinator 并返回 `WindowsFetchDataTransfer`，不会消耗 terminal ownership；调用方之后仍必须 `complete` 或 `fail`。普通 worker 应直接调用带 `WindowsFetchDataWaiterRegistry` 的 `hydrate`，由 request 保证所有 setup/backend/contract error 都先映射成 terminal CFAPI failure。success、failure、platform cancellation、watchdog 和 Drop 共用原子 terminal gate；registry 持有同一 gate，在 callback thread 内先发布 platform-cancelled 状态，再唤醒 core waiter。因此 cancel 后立刻 abort hydration task 时，request Drop 也不会伪造 `ProviderTerminated` completion。首次 native attempt 前即消耗 gate，即使 `CfExecute` 自身报错，Drop 也不会重复提交。

### Active waiter cancellation registry

`WindowsSyncRootConnection::fetch_data_waiters` 返回当前 connection 使用的 shareable registry。worker 必须把同一个 registry 传给 `WindowsFetchDataRequest::hydrate`；native `CANCEL_FETCH_DATA` callback 会在 callback thread 内只做 bounded HashMap matching 和 core cancellation-handle 调用，不执行 network I/O、数据库事务或阻塞 channel send。实际取消不依赖观察 queue 是否有容量；随后进入 `WindowsCallbackRequest::CancelFetchData` 的 snapshot 只用于日志、metrics 或诊断观察。

registry correlation 同时包含：

```text
SessionGeneration
ConnectionKey
TransferKey
RequestKey
FileId
```

取消 range 完整覆盖 active waiter range 时，registry 才调用该 waiter 的 `HydrationCancellationHandle::cancel`。如果只部分重叠，waiter 会被保留，因为 core waiter 当前是原子取消粒度，而 CFAPI 明确要求 cancel range 外的 bytes 继续完成。同一 correlation 下存在多个 range waiter 时，subset cancellation 只释放被完整覆盖的 waiter，其他 waiter 以及 coordinator 中仍被共享的 backend work 继续运行。后续若需要在单个大 waiter 内进一步停止 partial network read，应与多段 `TRANSFER_DATA` 和动态 range repartition 一起实现，不能通过粗暴取消整个 waiter 冒充支持。

`WindowsFetchDataWaiterRegistry::metrics` 提供 active/registered waiter、cancellation callback、unmatched、full/partial match、cancelled、repeated cancellation 和 completion-race counters。它们是 process-local mechanics，不带账号、空间或产品业务标签。

### Callback watchdog 与 progress

CFAPI callback 的固定 timeout 是 60 秒。Forge 暴露 `WINDOWS_CFAPI_CALLBACK_TIMEOUT` 与 `WindowsFetchDataWatchdogConfig`；配置必须为正数且不能长于平台 deadline。registry 在 waiter 注册时创建 `std::time::Instant` deadline，host/runtime 通过 `WindowsFetchDataWaiterRegistry::poll_watchdog(now)` 定期驱动，不在 callback thread 启动 Tokio timer、后台线程或阻塞等待。watchdog 赢得 core cancellation race 时，portable preparation 返回 `TimedOut`，与平台 `CANCEL_FETCH_DATA` 产生的 `Cancelled` 分开；普通 `hydrate` 会继续提交一次 terminal failure，重复 preparation 不会重新注册 waiter。

产品 worker 可在把 request 移入 hydration future 前创建：

```rust
let progress = request.progress_reporter();
```

`WindowsFetchDataProgressReporter` 只持有 generation、connection/transfer/request key 和 file ID 这些标量 correlation，不复制 callback 路径、identity、volume 或 process 字符串，也不持有 terminal completion authority，因此可低成本移动到另一个 worker。`WindowsFetchDataProgress::new(total, completed)` 验证 `completed <= total`、signed 64-bit native boundary、固定 total 和 monotonic completed；duplicate sample 合法，regression 与 total 变化在 native call 前拒绝。Windows target 上 `report` 先刷新匹配 waiters 的 watchdog，再用一个最小 `unsafe` block 调用 `CfReportProviderProgress(ConnectionKey, TransferKey, total, completed)`。registry 中已无匹配 waiter 时返回 0，不在 terminal 后继续调用 native progress。

`WindowsFetchDataWaiterMetrics` 额外记录 watchdog timeout、progress callback 和实际 deadline refresh。`WindowsSyncRootConnection::queue_metrics` 返回当前 generation 的饱和累加 ingress counters：accepted、queued fetch、queued cancel observation、fetch queue full、cancel observation queue full、receiver disconnected、queued preflight/observation、对应 queue full、closing rejection、invalid snapshot rejection 和 callback panic。累加计数使用独立原子值，不再与 lifecycle state/active callback 共用一把 mutex；snapshot 是诊断视图，不承诺跨 counter 的事务一致性。fetch/preflight queue full/disconnected 必须提交 terminal failure；cancel observation queue full 只丢诊断事件，实际 registry cancellation 已在此前完成。

### Restart、recovery 与 native mutation observation

`WindowsFetchDataSnapshot::is_recovery()` 表示 CFAPI 正在恢复 provider/system 非正常退出前中断的 hydration。产品 adapter 先用 stable identity 对 durable metadata、cache coverage 和 backend revision 做 reconciliation，然后选择继续 `hydrate` 或调用 `restart_hydration`。replacement identity 必须仍解码为 callback 的同一个 `CloudItemKey`；native operation 使用 `CF_OPERATION_TYPE_RESTART_HYDRATION`，成功后由 CFAPI 重发 callback。产品仍负责恢复 revision、journal、cache bytes 和业务 conflict policy。

validate/dehydrate/delete/rename preflight 进入 `WindowsCallbackRequest::Preflight`。request 持有 non-cloneable generation lease 和 exactly-once ACK responsibility；产品只有在自己的 intent/decision 已经耐久后才调用 `approve`，或使用结构化 failure 调用 `deny`。queue full、receiver disconnected、closing、panic 和未响应 Drop 会提交对应 `ACK_DATA`/`ACK_DEHYDRATE`/`ACK_DELETE`/`ACK_RENAME` failure。

dehydrate/delete/rename completion 进入 `WindowsCallbackRequest::Observation`。target/source path 在 callback 返回前复制为 owned `PathBuf`，stable identity 仍来自 `WindowsFileIdentity`。产品把 completion 映射到 core `MutationOrigin::PlatformObserved` 或 content eviction effect，并依靠启动 reconciliation 覆盖 observation queue full 或 journal 前退出窗口。Forge 不生成产品 operation ID、remote mutation 或冲突展示。

provider 发起的物理操作使用 `unregister_sync_root`、`set_placeholder_pin_state`、`set_placeholder_in_sync` 和 `dehydrate_placeholder`。内部通过 RAII 配对 `CfOpenFileWithOplock`/`CfCloseHandle`，handle 与 pointer 不跨同步调用。

### Windows 内存云盘 example

```powershell
cargo run -p aster_forge_cloud_files_windows --example memory_cloud_drive -- `
  C:\AsterForgeMemoryCloud
```

example 使用 `PopulationPolicy::AlwaysFull`，启动时创建 `hello.txt`、`readme.txt` 和 16 KiB `numbers.bin` placeholder；内容只存在进程内存，通过真实 `FETCH_DATA -> HydrationCoordinator -> TRANSFER_DATA` 回源。

sync-root registration 和 placeholder 是 Windows 持久状态，`CfConnectSyncRoot` callback connection 则只在 example 进程存活期间有效。关闭 example 后再访问尚未 hydration 的文件时，本地没有内容且没有在线 provider 可以处理 `FETCH_DATA`，Windows 会返回：

```text
ERROR_CLOUD_FILE_PROVIDER_TERMINATED (0x80070194)
The cloud file provider exited unexpectedly.
```

这是预期的 provider lifecycle 行为，也说明 Windows 已将该文件识别为 cloud placeholder；它不是 placeholder 或 hydration 数据损坏。重新运行 example 会再次调用 `CfConnectSyncRoot`，未下载文件随后可重新触发 hydration。已经 hydration 且仍保留本地内容的文件不需要在线 provider 即可读取；释放本地内容后则重新依赖 provider。测试结束后可清理持久 registration：

```powershell
cargo run -p aster_forge_cloud_files_windows --example memory_cloud_drive -- `
  --unregister C:\AsterForgeMemoryCloud
```

这是 platform integration fixture，不包含认证、远端 API、数据库、安装器或产品 UI。

本批 `WindowsFetchDataRequest::fail` 支持：

```text
InvalidRequest
InsufficientResources
ProviderTerminated
AuthenticationFailed
AccessDenied
NotInSync
NetworkUnavailable
NotSupported
Cancelled
Unsuccessful
```

它们映射为对应 `STATUS_CLOUD_FILE_*`；core backend authentication、permission、revision drift、temporary outage 和 unsupported classification 会映射到更具体的 CFAPI status。成功与失败都使用一次 `CF_OPERATION_TYPE_TRANSFER_DATA`。Windows 上 request 未显式终结就被 drop 时，会自动尝试 `ProviderTerminated`；queue full、receiver disconnected、session closing、callback panic、hydration backend error 和 response contract drift 也有明确失败路径。`CANCEL_FETCH_DATA` 是 cancellation notification，本身没有独立 terminal operation；它会直接命中 active waiter registry，完整覆盖时取消 core waiter，部分覆盖时保留仍承载 required bytes 的 waiter。

## Error boundary

`WindowsCloudFilesError` 区分：

- identity 超过 4 KiB；
- sync-root identity 超过 64 KiB；
- identity envelope malformed/unsupported；
- sync-root identity envelope malformed/unsupported；
- core identity value invalid；
- identity 与 placeholder item 不匹配；
- root file identity 与 sync-root scope 不匹配；
- provider name/version empty、NUL 或超过 255 UTF-16 units；
- provider GUID 为零；
- hydration policy conflict；
- policy 所需 platform integration 不足；
- sync-root path invalid；
- native CFAPI structure size overflow；
- Windows placeholder name invalid；
- core item metadata 不能映射为 Windows placeholder；
- file size 超过 signed CFAPI boundary；
- wide path embedded NUL；
- placeholder batch count overflow；
- callback range、priority、signed file size、identity scope 或 native structure invalid；
- fetch transfer range/alignment/response contract invalid；
- core hydration/backend failure；
- stale callback/request generation；
- connection 不再 accepting；
- 非法 connection lifecycle transition；
- active callback count overflow；
- active fetch waiter identity/count overflow；
- provider progress overflow、completed 超过 total、total 变化或 completed regression；
- watchdog timeout 配置非法或 pending fetch 超时；
- Windows target 上的 native CFAPI error。

产品层继续负责把这些错误映射为自己的日志字段、metrics、错误 envelope 和用户提示。

## 测试要求

```bash
cargo test -p aster_forge_cloud_files_windows
cargo clippy -p aster_forge_cloud_files_windows --all-targets -- -D warnings
cargo clippy -p aster_forge_cloud_files_windows \
  --all-targets \
  --target x86_64-pc-windows-gnu \
  -- -D warnings
```

当前 contract tests 覆盖：

- identity Unicode、delimiter、exact-byte round trip；
- rename/path independence；
- namespace/root collision isolation；
- exact 4096-byte acceptance 和 4097-byte rejection；
- truncated/magic/version/length/trailing/UTF-8/empty-field errors；
- debug redaction 与 owned byte consume；
- file/directory metadata mapping；
- signed file-size boundary；
- identity/item mismatch；
- placeholder options/accessors/consume；
- Windows reserved characters、trailing dot/space 和 device names；
- similar non-device names，例如 `COM0`、`COM10`、`CONSOLE`。
- sync-root identity Unicode/exact-byte round trip 和 debug redaction；
- exact 65536-byte acceptance 和 65537-byte rejection；
- malformed sync-root magic/version/length/trailing/UTF-8/empty-field；
- provider GUID non-zero/canonical value；
- provider name/version empty、NUL 和 255/256 UTF-16 boundary；
- root file identity same-scope binding；
- hydration validation/streaming conflict；
- full-restart exact `0x500` integration gate；
- unrestricted placeholder-management exact `0x310` integration gate；
- every exposed hydration/population/hard-link policy shape；
- all in-sync tracking bits、registration flags、accessors 和 owned consume。
- native key 完整 signed scalar domain；
- exact range、`CF_EOF`、zero/negative/overflow range boundary；
- known/future fetch 与 cancel flag bits；
- callback info 全字段 accessor、owned strings/bytes、scope validation 和 debug redaction；
- priority `15/16`、signed file-size 和 cross-scope identity boundary；
- fetch/cancel snapshot 与受控 request constructor；
- snapshot/lease generation mismatch；
- complete session transition matrix、duplicate close/disconnect state 和 immediate close；
- closing 后并发 callback 全部拒绝；
- accepted lease drain、request drop 和 old/new generation completion fence；
- default connect options 和 Windows-only public API signature/`Send` contract。
- hydration request key/revision/size/generation/alignment binding；
- exact、`CF_EOF`、empty、overflow、unaligned 和 beyond-file transfer range；
- success response revision/offset/length/total-size contract；
- coordinator success/backend failure bridge 和 terminal-gate ownership；
- all core hydration error classifications 到 CFAPI failure 的映射；
- success/failure operation buffer、keys、range、status 与 exact `ParamSize`。
- cancellation full-cover、partial-overlap、no-overlap 与 invalid range；
- generation/native-key correlation isolation；
- 同 correlation 多 range waiter 的 subset cancellation；
- repeated cancellation、completion-wins race 和 terminal suppression；
- registry identity/count overflow 不产生半注册状态；
- active waiter/cancellation metrics accessor 与 unregister cleanup。
- 60 秒上限、zero/overflow progress、monotonic/duplicate/regression validation；
- progress deadline refresh、watchdog due boundary、timeout/cancel classification 和重复 preparation；
- watchdog cancellation/completion race、progress mutation atomicity 与 registry metrics；
- callback ingress 各分支 counters 与 `u64::MAX` saturation；
- Windows-only progress reporter、`CfReportProviderProgress` 和 queue-metrics public signatures。
- recovery flag、restart metadata/identity options 和 same-item identity fence；
- validate/dehydrate/delete/rename owned preflight 与 completion snapshot；
- generic callback union member exact `ParamSize`/truncation boundary；
- callback table ordering/sentinel 与 every registered callback pointer；
- unregister、pin、in-sync、dehydrate 和 memory example Windows-only signatures/build。

Windows target compile/Clippy 负责验证 windows-rs feature、native struct layout usage、wide string lifetime、callback registration sentinel、`CfCreatePlaceholders`、`CfGetPlatformInfo`、`CfRegisterSyncRoot`、`CfConnectSyncRoot`、`CfExecute`、`CfReportProviderProgress`、`CfOpenFileWithOplock`、`CfSetPinState`、`CfSetInSyncState`、`CfDehydratePlaceholder` 和 `CfDisconnectSyncRoot` signature。真实 CFAPI integration tests 后续在 Windows runner 上覆盖 persistent registration/query/update、callback concurrency、disconnect-during-callback、queue rejection completion、placeholder filesystem observation、partial batch result 和 provider restart。macOS 上的 cross-target check 不冒充这些 runtime tests。

CI 的 `windows-cloud-files` job 会在 `windows-latest` 原生执行 portable contract 与 Windows-only synthetic callback tests，覆盖 truncated `StructSize` / `ParamSize`、真实 union padding、owned context、identity/string/process snapshot。Miri 使用固定 nightly 对 core 和 Windows crate 的 portable 路径运行；由于 Ubuntu 构建不会编译 `cfg(windows)` native module，Miri 不代表 CFAPI trampoline 已执行。该部分由 Windows synthetic tests、Windows Clippy 和后续真实 sync-root integration runner 分层验证。

## Windows Phase 2 状态

1. ~~connection/session generation、callback registration、owned snapshot、bounded ingress 与失败终结。~~
2. ~~`FETCH_DATA -> HydrationCoordinator -> TRANSFER_DATA` range 闭环和 exactly-once terminal gate。~~
3. ~~active waiter registry、request/range correlation、range-subset cancellation 和 in-flight metrics。~~
4. ~~timeout/watchdog、progress reporting 和 callback queue metrics。~~
5. ~~restart hydration 和 provider crash recovery callback fence。~~
6. ~~validate/rename/delete/dehydrate ACK、completion observation 与 native physical effects。~~
7. ~~可运行的 Windows in-memory cloud drive integration fixture。~~

Windows product-neutral PoC 已收口。真实机器 runner 继续验证 Explorer 行为、平台版本差异、callback concurrency、provider kill/restart 和 filesystem observation；发现 native contract 偏差时回到 Windows adapter 修正，不通过 AsterDrive endpoint 或 DTO 反向决定 Forge API。

## 参考

- [`aster_forge_cloud_files_core`](./aster_forge_cloud_files_core.md)
- [CfCreatePlaceholders](https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfcreateplaceholders)
- [CfRegisterSyncRoot](https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfregistersyncroot)
- [CfConnectSyncRoot](https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfconnectsyncroot)
- [CfDisconnectSyncRoot](https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfdisconnectsyncroot)
- [CF_CALLBACK_INFO](https://learn.microsoft.com/windows/win32/api/cfapi/ns-cfapi-cf_callback_info)
- [CF_CALLBACK_PARAMETERS](https://learn.microsoft.com/windows/win32/api/cfapi/ns-cfapi-cf_callback_parameters)
- [CfExecute](https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfexecute)
- [CfReportProviderProgress](https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfreportproviderprogress)
- [CF_CALLBACK_TYPE](https://learn.microsoft.com/windows/win32/api/cfapi/ne-cfapi-cf_callback_type)
- [CF_OPERATION_PARAMETERS](https://learn.microsoft.com/windows/win32/api/cfapi/ns-cfapi-cf_operation_parameters)
- [CfDehydratePlaceholder](https://learn.microsoft.com/windows/win32/api/cfapi/nf-cfapi-cfdehydrateplaceholder)
- [CF_SYNC_REGISTRATION](https://learn.microsoft.com/windows/win32/api/cfapi/ns-cfapi-cf_sync_registration)
- [CF_SYNC_POLICIES](https://learn.microsoft.com/windows/win32/api/cfapi/ns-cfapi-cf_sync_policies)
- [CF_PLACEHOLDER_CREATE_INFO](https://learn.microsoft.com/windows/win32/api/cfapi/ns-cfapi-cf_placeholder_create_info)
- [CF_PLACEHOLDER_MAX_FILE_IDENTITY_LENGTH](https://learn.microsoft.com/windows/win32/api/cfapi/ns-cfapi-cf_placeholder_create_info)
