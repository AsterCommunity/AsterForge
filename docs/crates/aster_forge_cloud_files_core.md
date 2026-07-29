# aster_forge_cloud_files_core

`aster_forge_cloud_files_core` 提供 Windows Cloud Files、Apple File Provider 和 Linux FUSE 之间共享的产品无关值模型与 capability negotiation。它把稳定身份、metadata/content revision、分页/变化/directory continuation token，以及 hydration、materialization、mutation、cancellation 和 naming 能力放进操作系统无关合同。

这个 crate 不是 AsterDrive SDK，也不是某个云盘 HTTP client。它不包含 endpoint、DTO、认证、账户、个人/团队空间、权限、产品冲突文案或 native platform type。产品仓库负责实现 backend adapter；后续 `aster_forge_cloud_files_windows`、`aster_forge_cloud_files_macos_bridge` 和 `aster_forge_cloud_files_linux` 负责平台映射。

当前 Phase 1 已完成 Batch 1～6，并在 Linux Phase 4 Batch 2C～2E 补齐 runtime-neutral resumable upload runner 与 generic mutation runner：除基础 value model 外，已经包含 item/page/change/reset、revision-bound whole/range read、backend error classification、临时只读 backend ports、durable cursor checkpoint、mutation journal/session fence ports、runtime-neutral hydration coordinator、content-storage ownership/sparse coverage、recoverable provider-cache writes、immutable local dirty generations、resumable upload backend/store/runner、generic mutation backend/store/runner、lease/dirty/pin eviction guards、durable eviction recovery，以及 Full/Limited/ReadOnly synthetic profiles。当前 backend/store/coordinator API 仍是 executable-model contract；native platform binding 会在对应 PoC 建立后进入后续阶段。

## 适用场景

- 用 namespace/root/item 组成稳定、与路径无关的云端身份。
- 分开表示 metadata revision、content revision 和可选 content digest。
- 分开表示 backend `PageCursor`、durable `ChangeCursor` 和 native `DirectoryCookie`。
- 描述 backend、platform 和 product host 的能力及 quantitative limits。
- 计算三层能力交集，得到一个 session 的 effective capabilities。
- 实现产品侧 metadata/content backend adapter，并通过 synthetic conformance suite 验证。
- 表达 paged enumeration、anchored changes、delete tombstone 和 explicit cursor reset。
- 发起绑定 `ContentRevision` 与 metadata size 的 whole/range content read。
- 在 active cursor 前持久化 change batch，并在 effects replayable 后原子提交 cursor。
- 持久化 command/preflight/observed mutation intent、remote outcome 和 session generation fence。
- 合并相同或重叠的 revision-bound hydration read，并提供 waiter-scoped cancellation。
- 分开记录 provider cache sparse ranges 与 platform-observed materialization state。
- 在 physical bytes 可观察后提交 provider coverage，支持 cache-write crash replay。
- 将本地修改捕获成 immutable `LocalContentSnapshot`，用 generation fence 防止旧上传清除新修改。
- 使用调用方 executor 驱动 resumable upload session/chunk/commit，并通过 idempotency/status reconciliation 恢复 remote outcome unknown。
- 使用同一个通用 mutation runner 推进 create、metadata/content mutation 与 delete 的 durable apply/reconcile/platform completion 状态机。
- 在 pin、dirty、open/read/write/hydration/upload/verification lease 保护下执行可恢复 eviction。
- 在 Windows adapter 编码 CFAPI `FileIdentity` 后检查 4 KB 等 native limit。

不适合放在这里的内容：

- AsterDrive API、DTO、entity、repository 和 token refresh。
- 产品 root 的个人/团队/账户含义。
- 产品权限、quota 展示、错误文案和冲突副本名称。
- CFAPI、Swift/Objective-C 或 FUSE native request type。
- Windows executable、macOS extension target 或 Linux daemon packaging。

## Cargo 接入

```toml
[dependencies]
aster_forge_cloud_files_core = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag，也不依赖具体异步 runtime、数据库或网络 client。只读 backend、upload/mutation backend、immutable snapshot reader 与 durable store port 使用 `async-trait` 提供 object-safe dispatch；hydration coordinator 使用 `futures` shared future/channel，在调用方 executor 上被 waiter 驱动，不自行创建 Tokio runtime 或后台 task。`ContentUploadRunner` 与 `MutationRunner` 同样不 spawn、不 sleep、不创建 runtime，只在调用方 future 中推进 durable record。content-storage public model 只描述 ownership、coverage、guard 和 durable transition，不创建缓存文件，也不调用 native eviction API。upload chunk 当前使用 `bytes::Bytes`；transaction adapter 和 lease registry 仍由产品 store 实现。

## Scoped identity

完整身份由三层组成：

```rust
use aster_forge_cloud_files_core::{
    CloudFilesCoreError, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};

fn identity_example() -> Result<(), CloudFilesCoreError> {
    let scope = CloudScope::new(
        CloudNamespaceId::new("provider-domain")?,
        CloudRootId::new("remote-root-id")?,
    );
    let key = CloudItemKey::new(
        scope,
        CloudItemId::new("stable-item-id")?,
    );

    assert_eq!(key.item_id().as_str(), "stable-item-id");
    Ok(())
}
```

`CloudItemKey` 不包含 path、filename、inode 或 native identity blob。rename 和同 root move 只更新外部 item metadata，key 保持不变。两个 namespace 或 root 即使复用了相同 remote item ID，也会得到不同的 `CloudItemKey`。

identity constructor 只拒绝空字符串，不 trim、不规范化，也不解释产品含义。adapter 需要保留 backend identity 的精确值；产品自己的格式限制继续留在产品边界。

## Item 与目录分页

`CloudItem` 将稳定身份和当前 metadata 分开：

```rust
use aster_forge_cloud_files_core::{
    CloudContentMetadata, CloudItem, CloudItemId, CloudItemKey, ContentRevision,
    MetadataRevision,
};

fn file_item(
    key: CloudItemKey,
    parent_id: CloudItemId,
) -> aster_forge_cloud_files_core::Result<CloudItem> {
    CloudItem::file(
        key,
        parent_id,
        "report.pdf",
        MetadataRevision::from_slice(b"metadata-v3")?,
        CloudContentMetadata::new(
            ContentRevision::from_slice(b"content-v8")?,
            None,
            4096,
        ),
    )
}
```

当前 item kind 只包含 `File` 和 `Directory`。symlink、package、alias 等平台类型等对应平台合同和 PoC 证明共享语义后再进入 core。

结构约束：

- root item 是 `parent_id = None` 的 directory；
- file 必须有 parent 和 `CloudContentMetadata`；
- directory 没有 content metadata；
- item 不能把自己的 ID 作为 parent；
- root item不能通过普通 same-root move 下挂到另一 parent；
- `moved()` 只更新 parent/name/metadata revision，key 和 content revision 保持不变。

目录枚举使用：

```rust
pub struct CloudItemPage {
    // items + optional next PageCursor
}
```

`next_cursor = None` 表示本次枚举结束。cursor 只对产生它的 parent enumeration 有效；adapter 应把跨 parent、损坏或过期的 page cursor 映射为 `InvalidRequest` 或对应 backend failure。

## Revision 与 digest

```rust
use aster_forge_cloud_files_core::{
    CloudFilesCoreError, ContentDigest, ContentDigestAlgorithm, ContentRevision,
    MetadataRevision,
};

fn revision_example() -> Result<(), CloudFilesCoreError> {
    let metadata_revision = MetadataRevision::from_slice(b"metadata-v3")?;
    let content_revision = ContentRevision::from_slice(b"content-v8")?;
    let digest = ContentDigest::new(
        ContentDigestAlgorithm::new("sha-256")?,
        vec![0x42; 32],
    )?;

    assert_ne!(metadata_revision.as_bytes(), content_revision.as_bytes());
    assert_eq!(digest.algorithm().as_str(), "sha-256");
    Ok(())
}
```

revision 是 opaque equality/precondition token，没有排序语义。strong ETag 可以作为 `ContentRevision`，但只有 backend 明确提供某种摘要算法的真实 digest bytes 时才构造 `ContentDigest`。

## Cursor 边界

三类 continuation token 使用三个不同 Rust 类型：

```text
PageCursor      -> backend list pagination
ChangeCursor    -> durable remote change checkpoint
DirectoryCookie -> one native open-directory stream
```

初始位置使用 `None`，不使用空 token。类型没有互相转换 API，避免把 FUSE `readdir` offset 持久化成远端 change cursor。

## Change feed 与 reset

远端变化由两类 effect 表达：

```rust
pub enum CloudChange {
    Upsert { item: CloudItem },
    Delete {
        key: CloudItemKey,
        previous_parent_id: Option<CloudItemId>,
        metadata_revision: Option<MetadataRevision>,
    },
}
```

rename/move 使用相同 `CloudItemKey` 的 `Upsert` 表达。core/store 后续根据 durable old snapshot 和新 item 计算 old parent、new parent、item 及 working-set invalidation。delete 使用 tombstone，不要求返回已经不存在的完整 item。

`ChangeBatch` 包含：

- ordered changes；
- `next_cursor`；
- `has_more`。

`next_cursor` 只有在 batch 和后续 platform effect 已经耐久、可重放后才能成为 active cursor。Batch 3 使用 `ChangeCheckpointStore` 固定了这个两阶段提交顺序。

cursor continuation 失败使用显式结果：

```rust
pub enum ChangePage {
    Batch(ChangeBatch),
    ResetRequired { reason: ChangeResetReason },
}
```

reset reason 当前区分：

- `CursorExpired`：执行 full reconciliation 并建立新 baseline/cursor；
- `BackendStateRebuilt`：backend change-tracking state 已替换；
- `IdentityMappingInvalidated`：item identity mapping 也不再可信，平台 adapter 可能需要 reimport。

## Durable cursor checkpoint

`ChangeCheckpointStore` 不提供“直接覆盖 active cursor”的捷径。一个 change batch 必须依次经过：

```text
record_change_batch
    ↓ batch + base cursor durable; active cursor unchanged
mark_change_effects_replayable
    ↓ item/platform effects can be replayed after restart
commit_change_cursor
    ↓ atomically promote next cursor and clear pending batch
```

核心记录类型：

```rust
use aster_forge_cloud_files_core::{
    ChangeBatch, ChangeBatchId, CloudScope, PersistedChangeBatch,
};

fn durable_batch(
    scope: CloudScope,
    batch: ChangeBatch,
) -> aster_forge_cloud_files_core::Result<PersistedChangeBatch> {
    Ok(PersistedChangeBatch::new(
        ChangeBatchId::new("change-batch-42")?,
        scope,
        None,
        batch,
    ))
}
```

`ChangeCursorCheckpoint` 同时返回 `active_cursor` 与可选 `pending_batch`。如果进程在 batch persist 后、cursor commit 前退出，恢复流程继续使用旧 active cursor，并重放 pending batch；它不会跳过尚未形成 replay guarantee 的变化。

`commit_change_cursor` 对相同 `ChangeBatchId` 幂等。真实数据库实现负责把 cursor promotion、pending clear 和 committed batch identity 写入同一个本地事务；SQL/ORM entity 与历史 migration 仍由产品仓库拥有。

## Mutation journal 与 session fence

mutation ingress 保留三类平台语义：

```text
PlatformCommand   -> native operation waits for durable acceptance/outcome
PlatformPreflight -> native operation waits for allow/deny/ack
PlatformObserved  -> platform effect already happened; journal + reconcile
```

`MutationOrigin::RemoteChange` 供 remote delta 到 platform state 的 durable reconciliation 使用。它不把 backend change feed 伪装成本地用户 mutation。

`MutationIntent` 至少携带：

- `OperationId` 与 `IdempotencyKey`；
- `MutationOrigin` 与 `SessionGeneration`；
- `DesiredMutation`；
- 分离的 metadata/content revision precondition；
- 可选 immutable `LocalContentSnapshot`；
- 可选 owned platform request correlation；
- 可选 remote reconciliation key；
- retry attempt 与 `not_before`。

native pointer、borrowed callback object、FUSE reply handle 等不得写进 intent。只有 `DesiredMutation::ModifyContent` 可以携带 local snapshot；create、metadata-only mutation 和 delete 附带 snapshot 会被视为无效 intent，避免把与操作无关的本地内容状态写入 journal。content mutation 使用的 snapshot 同时携带 item key、non-zero local generation、opaque local reference、size 与可选 digest；被引用的 bytes 在该 generation 生命周期内保持 immutable，并且 snapshot item key 必须与 mutation target 精确一致。adapter 需要在跨 async/durable boundary 前生成 owned immutable correlation token。

初始构造的 `MutationIntent` 仍在内存中。只有 `persist_mutation_intent` 成功落下 `MutationRecord { state: IntentPersisted }` 后，command-driven ingress 才具备可恢复保证。`PlatformObserved` 在 journal 前退出的窗口由平台 dirty/pending 状态或本地 metadata startup reconciliation 重新发现，core 不把 `Detected` 伪造成 durable state。

durable state machine：

```text
IntentPersisted
  -> RemoteApplying
  -> RemoteOutcomeUnknown | RemoteOutcomeKnown
  -> PlatformReconciled
  -> Completed
```

`RemoteOutcomeUnknown` 是恢复分类，不是普通“再发一次就行”的网络错误。恢复端需要使用 idempotency key、remote status query 或 change feed，把它解析为：

- `Committed { item }`
- `AlreadyCommitted { item }`
- `PreconditionFailed { metadata_revision, content_revision }`

delete 的 committed/already-committed outcome 可以使用 `item: None`，不会强迫 backend 返回已经删除的对象。

所有 durable marker 都按“相同事实可重放、不同事实冲突”处理。operation 已经进入更晚状态甚至 `Completed` 后，重复相同的 remote-apply、remote-outcome、platform-reconciled 或 completion marker 返回 `AlreadyApplied`，用于覆盖数据库提交成功但调用方没有收到返回值的窗口；晚到的不同 remote outcome 仍然是 transition conflict，不能覆盖已经持久化的结果。

`MutationJournalStore` 把 session lifecycle 固定为：

```text
Accepting -> Closing -> Draining -> Closed
```

`Closing` 后的新 intent 被 fence。mutation completion 必须带当前执行者的 `SessionGeneration`；更高 generation 激活后，旧 generation 的晚到 completion 返回 `StoreWriteStatus::Fenced`，不得修改新 session 的 active state。新 generation 的 startup recovery 可以显式继续旧 intent，因为 immutable intent 仍保留最初捕获的 generation，而每次 transition 另外携带当前 active generation。产品/platform adapter 继续负责 native connection、extension 或 mount 的真实关闭、取消和 drain 策略。

当前 deterministic `MemoryCloudFilesStore` 只存在于 `tests/support/`，用于验证所有 durable boundary。生产 crate 不包含把内存模型伪装成数据库的 test scaffolding。

### Generic mutation runner

`MutationRunner` 把 journal contract 收成一个 runtime-neutral 执行器：

```text
durable MutationIntent
-> generation-aware begin_remote_apply
-> CloudMutationBackend::apply_mutation
-> known outcome | RemoteOutcomeUnknown
-> CloudMutationBackend::reconcile_mutation
-> generation-aware product/platform reconciliation
-> completion
```

产品实现 `CloudMutationBackend`，负责 endpoint、认证、DTO、revision precondition、idempotency/status query 和 stable/provisional item identity 映射。`apply_mutation()` 必须使用 intent 已有的 `IdempotencyKey`；transport 已发出请求但无法证明是否提交时，返回 `RemoteOutcomeUnknown`，而不是把它降成普通 backend error。后续 invocation 只在 durable state 已经是 unknown 时调用 `reconcile_mutation()`。

```rust
use aster_forge_cloud_files_core::{
    CloudMutationBackend, MutationIntent, MutationJournalStore, MutationRunOutcome,
    MutationRunner, SessionGeneration,
};

async fn resume_mutation(
    intent: MutationIntent,
    generation: SessionGeneration,
    store: &dyn MutationJournalStore,
    backend: &dyn CloudMutationBackend,
) -> aster_forge_cloud_files_core::MutationRunResult<MutationRunOutcome> {
    MutationRunner
        .submit(intent, generation, store, backend)
        .await
}
```

`MutationJournalStore::mark_platform_reconciled()` 是产品 durable reconciliation boundary，不是空 marker。实现必须在同一个 active-generation transaction 中完成或耐久排入所需 local metadata、namespace mapping 与 platform effect；无法与数据库事务合并的 native invalidation 应先形成可重放的 durable effect/queue，再标记 reconciled。core runner 不直接调用 CFAPI、File Provider 或 FUSE API。

runner 会验证 known outcome 的基本产品无关结构：create 必须返回匹配 scope/parent/name/kind 的 item；modify-content 必须返回同 key 的 file；metadata mutation 必须返回同 key 且反映显式请求的 parent/name；delete 可以返回 `item: None`。create 的最终 stable key 或 provisional/final mapping 仍由产品 store reconciliation 根据 operation identity 校验，core 不从 pathname 推导该 identity。

`MutationRunOutcome` 分为：

- `Completed`：known outcome、产品/platform reconciliation 与 completion 都已耐久；
- `RemoteOutcomePending`：status reconciliation 仍无法证明远端结果，由产品 scheduler 决定 backoff 后再次调用 `resume()`；
- `Fenced`：executor generation 已过期，由新 session generation 恢复同一 durable intent。

## Capability negotiation

`CloudFilesCapabilities` 当前包含：

- `identity`
- `revisions`
- `enumeration`
- `changes`
- `hydration`
- `mutations`
- `materialization`
- `eviction`
- `cancellation`
- `naming`
- `limits`

调用方分别描述 backend、platform 和 product host，然后连续求交：

```rust
use aster_forge_cloud_files_core::CloudFilesCapabilities;

fn effective_capabilities(
    backend: CloudFilesCapabilities,
    platform: CloudFilesCapabilities,
    host: CloudFilesCapabilities,
) -> aster_forge_cloud_files_core::Result<CloudFilesCapabilities> {
    backend.intersection(platform)?.intersection(host)
}
```

`CloudFilesCapabilities::default()` 表示全部 optional capability 都处于 unsupported 状态。某一层只约束部分维度时，可以从 `CloudFilesCapabilities::unconstrained()` 开始，再覆盖该层真实拥有的维度；这个 constructor 是 capability intersection 的 identity element，不代表某个真实实现已经证明支持全部能力。

可选操作缺少共同能力时，对应字段保持 unsupported 状态，不作为内部失败。例如双方没有共同 range hydration 时，`effective.hydration.range == None`。

三个 identity invariant 是 core 的硬边界：

- stable item ID；
- path-independent item ID；
- same-root move preserves identity。

组装 session 后调用：

```rust
effective.validate_core_requirements()?;
```

cross-root move 仍是可选能力。

### Range alignment

range capability 把 physical alignment 放在 `RangeHydrationCapabilities` 中，而不是把它伪装成 backend content API 的全局约束。两个 boundary 求交时使用 least common multiple：例如 4096 和 6144 的 effective alignment 是 12288。计算溢出返回结构化错误。

### Quantitative limits

`CloudFilesLimits` 当前包含：

- `native_identity_max_bytes`
- `max_transfer_chunk`
- `max_in_flight_requests`
- `directory_page_size_hint`
- `name_max_encoded_bytes`

`None` 表示该 boundary 没有附加限制；求交时采用双方更严格的非空 limit。Windows adapter 可以把 `native_identity_max_bytes` 设置为 4096，并在完成 native encoding 后调用 `validate_native_identity()`。

## Read-only backend contract

Phase 1 当前提供三个临时 port：

```rust
pub trait CloudMetadataBackend: Send + Sync {
    async fn get_item(...);
    async fn list_children(...);
    async fn changes_since(...);
}

pub trait CloudContentBackend: Send + Sync {
    async fn read_content(...);
}

pub trait CloudFilesBackend:
    CloudMetadataBackend + CloudContentBackend
{
    fn capabilities(&self) -> CloudFilesCapabilities;
}
```

这些 trait 当前 object-safe，便于 product host 使用 `dyn CloudFilesBackend` 组装 executable model。resumable upload 使用独立的 `CloudContentUploadBackend`，generic durable mutation 使用独立的 `CloudMutationBackend`，对应 checkpoint/journal 使用 `ContentUploadStore` 与 `MutationJournalStore`；它们没有塞进 read-only `CloudFilesBackend`，避免让只读 backend 被迫实现虚假的 mutation 方法，也避免把 backend transport、local database 和 platform completion 塞进一个万能 trait。

backend 返回自己的 constraints。该 backend 不负责限制的 capability 维度从 `CloudFilesCapabilities::unconstrained()` 开始；例如物理 materialization 通常由 platform/host 决定，而 change-feed 和 revision 能力主要由 backend 决定。

## Revision-bound content read

whole/range read 使用：

```rust
use aster_forge_cloud_files_core::{ByteRange, ContentReadRequest};

fn first_chunk(
    file: &aster_forge_cloud_files_core::CloudItem,
) -> aster_forge_cloud_files_core::Result<Option<ContentReadRequest>> {
    let Some(content) = file.content() else {
        return Ok(None);
    };
    Ok(Some(ContentReadRequest::range(
        file.key().clone(),
        content.revision().clone(),
        content.size(),
        ByteRange::new(0, 4096)?,
    )))
}
```

request 同时绑定：

- `CloudItemKey`；
- exact `ContentRevision`；
- metadata 中该 revision 的 expected size；
- `Whole` 或非空 `ByteRange`。

`ContentReadResponse` 包含 revision、logical offset、`bytes::Bytes` 和 total size。`validate_response()` 检查：

- response revision 与 request 完全一致；
- total size 与 metadata expected size 一致；
- whole response 从 offset 0 覆盖完整文件；
- range response 从 requested offset 开始；
- range 只允许在 EOF 截短；
- response bytes 不越过 logical file size。

range request offset 等于 EOF 时可以返回 empty bytes；offset 超过 EOF 由 backend 返回 `InvalidRequest`。stale revision 返回 `PreconditionFailed`，而不是悄悄读取当前版本。

上面的 API 仍是 Phase 1 executable model。`Bytes`、`async-trait`、stream、associated future 或 caller-owned buffer 的最终取舍会由平台热路径验证。

## Hydration coordinator

`HydrationCoordinator` 固定绑定一个 product-owned `CloudContentBackend`，避免不同 adapter 实例意外共享同一 in-flight work：

coordinator 默认限制同时保留的 backend work 数量（全局 1024、单 content key 128）。产品可通过 `HydrationCoordinator::with_limits` 为 extension/daemon 设置更小的 `HydrationLimits`；达到上限时 request 返回 `HydrationError::InFlightLimitExceeded`，等待现有 waiter 释放后再重试。

```rust
use aster_forge_cloud_files_core::{
    Alignment, ByteRange, ContentRevision, HydrationCoordinator, HydrationRequest,
    SessionGeneration,
};

fn range_waiter(
    coordinator: &HydrationCoordinator,
    key: aster_forge_cloud_files_core::CloudItemKey,
    revision: ContentRevision,
    alignment: Alignment,
) -> aster_forge_cloud_files_core::HydrationResult<
    aster_forge_cloud_files_core::HydrationWaiter,
> {
    coordinator.request(HydrationRequest::range(
        key,
        revision,
        8192,
        ByteRange::new(1024, 2048)?,
        alignment,
        SessionGeneration::new(7)?,
    ))
}
```

production code 需要从 effective `RangeHydrationCapabilities` 取得并传入 alignment。

共享 key 包含：

```text
CloudItemKey
+ ContentRevision
+ expected content size
+ SessionGeneration
```

因此：

- rename/path 变化不会拆散同一内容请求；
- 不同 namespace/root/item 不碰撞；
- 不同 content revision 不会混入同一个 assembly；
- metadata 对同一 revision 报出不同 size 时不会复用旧 work；
- provider restart 后的新 generation 不接管旧 generation 的 in-flight completion。

### Exact 与 overlapping range

相同 item/revision/size/generation/range 的多个 waiter 共享一个 backend future。

重叠 range 使用已有 work，只为尚未覆盖的 physical gap 创建新 backend read。例如：

```text
waiter A logical/physical: [0, 8)
waiter B logical/physical: [4, 12)

backend work:
  [0, 8)  shared by A + B
  [8, 12) owned only by B
```

每个 waiter 最终只收到自己的 logical bytes。跨多个 work segment 的 assembly 只拼接 requested extent，不读取或复制完整文件。

whole-file waiter 保持 `ContentReadRange::Whole`，因此只支持 whole-file read 的 backend 不会被 coordinator 强制转换为 range API。并发 whole waiter 同样共享一个 backend future。

### Physical alignment

alignment 只扩大 physical backend read，不改变 logical result。例如 logical `[3, 6)` 配合 4-byte alignment 会读取 physical `[0, 8)`，waiter 仍只收到 offset 3 的三个 bytes。

到 EOF 的 physical end 会截到 metadata expected size。logical range 从 EOF 开始时仍会发起 backend read，以验证 requested `ContentRevision`；它返回 empty bytes，但不会绕过 stale-revision precondition。

### Waiter-scoped cancellation

每个 `HydrationWaiter` 提供独立的：

```rust
HydrationCancellationHandle
```

取消一个 waiter 只释放它引用的 work：

- 其他 waiter 仍引用的 shared range 继续执行；
- 只有被取消 waiter 使用的 uncovered gap 会被释放；
- 最后一个 waiter 释放某个 work 后，shared backend future 被 drop；
- completion 与 cancellation 使用原子 terminal race，只产生一个最终结果；
- completion 获胜后再次 cancel 返回 `AlreadyCompleted`。

future drop 是 coordinator 能提供的 runtime-neutral backend cancellation boundary。adapter 仍需按实际 transport/platform 能力声明 `CancellationLevel::None`、`BestEffortRequest`、`ExactRequest` 或 `RangeSubset`。core 的 waiter ownership 不会凭空把底层 transport 升级成 exact native cancellation。

当前 coordinator 不拥有：

- CFAPI transfer/completion；
- File Provider temporary URL 或 materialized copy；
- FUSE reply/inode/file handle；
- provider cache files 或 platform hydration bitmap；
- product retry/backoff、priority 或 bandwidth policy。

其中 sparse range metadata 已由下面的 content-storage mechanism 表达；真实 cache file、native materialization 和 physical effect 仍由 platform adapter/product host 执行。

## Content storage ownership 与 eviction recovery

`ContentCacheKey` 使用：

```text
CloudItemKey + ContentRevision
```

它不包含 path。rename 或同 root move 后，同一 revision 的 cache identity 保持不变；同一 item 的新 `ContentRevision` 一定形成新 entry。`ContentStorageMode` 明确三个物理 ownership：

- `PlatformManaged`：平台拥有 materialized copy；core 只保存 `PlatformMaterializationState` observation，不创建 provider range coverage。
- `ProviderManaged`：provider 拥有 backing cache；core 保存 normalized sparse `ContentRangeSet`，平台 materialization 字段为空。
- `Hybrid`：两层同时存在，但状态彼此独立。platform observation 不会把 bytes 记入 provider coverage。

`ContentRangeSet` 将 overlapping 或 adjacent byte ranges 归并成升序、互不重叠的 coverage。range end 不能超过 exact revision 的 metadata size；空文件在没有 range 的情况下也视为 complete。

最小 entry 形状：

```rust
use aster_forge_cloud_files_core::{
    CloudItemKey, ContentCacheKey, ContentRevision, ContentStorageEntry,
    ContentStorageMode,
};

fn provider_cache_entry(
    key: CloudItemKey,
    revision: ContentRevision,
    size: u64,
) -> ContentStorageEntry {
    ContentStorageEntry::new(
        ContentCacheKey::new(key, revision),
        ContentStorageMode::ProviderManaged,
        size,
    )
}
```

### Eviction guards

eviction reservation 按稳定顺序检查：

```text
Pinned
Dirty
OpenLease
ReadLease
WriteLease
HydrationLease
UploadLease
VerificationLease
EvictionInProgress
```

`ContentLeaseId + ContentLeaseKind` 让 acquire/release 可幂等重放。guard evaluation、entry reservation 和 intent persistence 必须序列化；reservation 落下后，新的 lease acquire 被拒绝，避免 reader/hydration 插入“检查完成但物理删除尚未开始”的窗口。

production store 可以把 durable entry/eviction metadata 放进数据库，同时把 open/read/hydration 等瞬时 lease 放在进程内 registry；`ContentStorageStore` 要求这两个部分在 eviction decision 上提供同一个 serialization boundary，并不要求把 native handle 或临时 lease ID 写入产品数据库。

### Ownership-aware eviction plan

`ContentEvictionTarget` 转换为显式、幂等的 physical effect plan：

| Storage mode / target | Remove provider cache | Evict platform materialization | Invalidate native/kernel content |
| --- | ---: | ---: | ---: |
| `PlatformManaged` / platform or all | no | yes | no |
| `ProviderManaged` / provider or all | yes | no | yes |
| `Hybrid` / provider | yes | no | yes |
| `Hybrid` / platform | no | yes | no |
| `Hybrid` / all | yes | yes | yes |

provider-cache removal 后仍要求显式 `PlatformContentInvalidated` marker。Linux kernel page cache、FUSE lookup/content view 或其他 native cached view 与 provider backing cache 是不同层；在确认没有相关 native state 时，adapter 可以执行幂等 no-op，但仍需记录 effect observation，不能把两个 ownership 层混为一谈。

durable eviction state machine：

```text
IntentPersisted
  -> PhysicalEffectsApplied
  -> MetadataReconciled
  -> Completed
```

`ContentStorageStore::begin_content_eviction` 原子完成 guard check、reservation 与 intent persist。之后 adapter 分别执行并观察：

- `ProviderCacheRemoved`
- `PlatformMaterializationEvicted`
- `PlatformContentInvalidated`

只有 plan 要求的全部 effect marker 都落下后，metadata reconciliation 才能清空 provider ranges、把 platform state 设为 `Dataless`。completion 最后释放 reservation。每个 transition 都支持相同输入幂等重放；同一个 operation ID 指向不同 intent 时返回 conflict。

如果进程在 physical effect 已发生、marker 尚未写入时退出，startup recovery 先观察真实物理状态，再补记 marker。例如 provider cache file 已不存在时，记录 `ProviderCacheRemoved`，而不是再次假定删除是否执行成功。completed record 不进入 `recoverable_content_evictions`。

deterministic `MemoryContentStorageStore`、failure injector 和 synthetic physical content 只存在于 `tests/support/`。生产 crate 没有内存数据库、缓存目录实现或 native API mock。

## Provider-cache write commit

hydration/backend read 成功只证明 bytes 已经到达内存，不代表 provider cache coverage 已经 durable。`ContentCacheWriteIntent` 把写入绑定到：

```text
ContentCacheWriteOperationId
+ ContentCacheKey
+ exact expected size
+ Whole | ByteRange
+ SessionGeneration
```

durable state machine：

```text
IntentPersisted
  -> PhysicalBytesCommitted
  -> CoverageCommitted
  -> Completed
```

顺序含义：

1. intent 与 entry cache-write reservation 原子提交；
2. adapter/host 把临时文件、sparse bytes 或 chunk 物理提交到最终 cache location；
3. physical bytes 可在 restart 后观察，才记录 `PhysicalBytesCommitted`；
4. `ContentCacheWriteStore` 原子更新 `ContentRangeSet` 与 `CoverageCommitted`；
5. completion 释放 cache-write reservation。

在 physical commit 前提交 coverage 会被拒绝。这样 crash 后不会出现“metadata 宣称 range 已缓存，实际临时文件还没 rename”的假 coverage。

cache-write reservation 作为独立 `CacheWriteInProgress` eviction blocker。它不是普通 runtime waiter：即使进程退出，durable write record 仍要求 startup 先观察临时/最终 physical state，再决定补记 physical marker、继续 coverage reconciliation，或重新执行幂等 physical commit。

`Whole` 支持空文件；size 为 0 时不会伪造非法的 zero-length `ByteRange`，而 `ContentRangeSet::is_complete(0)` 仍返回 true。

## Immutable dirty content 与 resumable upload

本地可变文件不能直接作为长期 upload source。Forge 使用：

```text
LocalContentReference
+ LocalContentGeneration
+ CloudItemKey
+ immutable size
+ optional real digest
= LocalContentSnapshot
```

`LocalContentGeneration` 在 item 内单调递增。记录 generation 2 后：

- generation 1 的重复 dirty observation 被 fence；
- generation 1 的 upload completion 被 fence；
- generation 1 的 remote commit 可以记录为已发生，但不会清除 generation 2 的 dirty snapshot。

`ContentUploadIntent` 同时绑定：

- mutation `OperationId`；
- backend `IdempotencyKey`；
- exact base `ContentCacheKey`，其中包含 conditional base `ContentRevision`；
- immutable local snapshot；
- operation-owned `Upload` lease；
- session generation。

upload transport 使用独立 port：

```rust
pub trait CloudContentUploadBackend: Send + Sync {
    async fn start_upload(...) -> BackendResult<ContentUploadSession>;
    async fn upload_chunk(...) -> BackendResult<ContentUploadChunkAck>;
    async fn commit_upload(...) -> BackendResult<MutationRemoteOutcome>;
    async fn reconcile_upload(...) -> BackendResult<MutationRemoteOutcome>;
}
```

immutable bytes 使用另一个独立 port，opaque local reference 的解释仍留在产品：

```rust
pub trait LocalContentSnapshotReader: Send + Sync {
    async fn read_snapshot(
        &self,
        snapshot: &LocalContentSnapshot,
        offset: u64,
        length: u64,
    ) -> StoreResult<Bytes>;
}
```

`ContentUploadSession` 保存 opaque backend session ID 和 accepted offset。checkpoint 只能单调前进；更小 offset 返回 fence。chunk 必须：

- 属于同一 backend session；
- 绑定 intent 的 exact local generation；
- 使用 snapshot exact total size；
- 从 durable accepted offset 开始；
- 获得 exact chunk-end acknowledgement。

remote commit 只有在 accepted offset 等于 snapshot size 后才能开始。durable upload state machine：

```text
IntentPersisted
  -> Uploading
  -> RemoteCommitting
  -> RemoteOutcomeUnknown | RemoteOutcomeKnown
  -> MetadataReconciled
  -> Completed
```

`Committed` / `AlreadyCommitted` upload outcome 必须带当前 revision-bearing `CloudItem`，并满足 item identity 与 snapshot size。content upload 不接受缺少 item metadata 的 committed outcome，因为后续 adapter 需要新的 metadata/content revision 继续同步。

accepted bytes 完整后，runner 先持久化 `RemoteCommitting` marker，再调用 backend 的幂等 conditional commit。进程在 marker 与 known outcome 之间退出时，`resume()` 可以用同一 intent/idempotency key 重发 commit；如果 transport 已经不能证明远端是否执行，backend 必须返回 `RemoteOutcomeUnknown`，后续 invocation 才使用 `reconcile_upload()` 查询 status/idempotency/change feed。这个流程不重新上传已 checkpoint 的 chunks，也不会把不确定结果当作普通 backend error。precondition failure 保留 dirty snapshot，交给 product conflict policy；committed outcome 只清除 exact matching local generation。

upload intent persistence 会原子验证 active dirty snapshot、获取 operation-owned `Upload` lease 并写入 record。completion 释放 lease。dirty 与 upload lease 都会阻止 eviction。

`ContentUploadRunner` 把上述 contract 组装成 runtime-neutral 执行器：

```rust
use aster_forge_cloud_files_core::{
    ContentUploadRunOutcome, ContentUploadRunner, SessionGeneration,
};

let runner = ContentUploadRunner::new(8 * 1024 * 1024)?;
let outcome = runner
    .submit(
        intent,
        SessionGeneration::new(active_generation)?,
        &product_upload_store,
        &product_upload_backend,
        &product_snapshot_reader,
    )
    .await?;

match outcome {
    ContentUploadRunOutcome::Completed => {}
    ContentUploadRunOutcome::RemoteOutcomePending => schedule_reconciliation(),
    ContentUploadRunOutcome::Fenced => stop_stale_executor(),
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

runner 会从 durable accepted offset 读取 exact snapshot range，校验 source byte count、chunk session/generation/offset/total size 与 backend exact ack，然后逐步落下 checkpoint、remote-commit marker、outcome、metadata reconciliation 和 completion。它遇到第一个 backend/store/contract failure 就返回，由产品根据 `RetryAdvice`、store error kind、lifecycle 和 backoff policy 决定何时再次 `resume()`；它自己不循环等待。

`ContentUploadStore` 所有 mutating transition 都接收本次执行者的 `SessionGeneration`。真实 store 必须在同一数据库事务内比较 active generation 并写 transition；较旧 mount/extension/connection 的晚到 checkpoint、outcome、metadata reconcile 和 completion 返回 `StoreWriteStatus::Fenced`。较新的 generation 可以继续旧 intent 的 immutable snapshot，旧 generation 只标识最初接收者，不阻止 startup takeover。

Linux writable adapter 不直接调用 backend transport。产品 `LinuxWritebackStore` 在 write/truncate 的 durable transaction 中保存 immutable snapshot 和调用方分配的 `ContentUploadIntent`，FUSE reply 后再由产品 worker 调用 runner。这样不会制造“snapshot 已回复成功、upload intent 尚未可恢复”的额外 crash window。Windows/macOS 也使用同一个 runner，只替换 snapshot reader、durable store 和 backend adapter。

### 有界 recovery page

四个 durable store trait 只提供带 `after_operation_id` 与 `usize` limit 的 `recoverable_*_page`。数据库实现必须在 SQL 层使用 `ORDER BY operation_id LIMIT`，避免启动时把整个 backlog 装入内存。

`CloudContentUploadBackend::reconcile_upload_chunk` 为 chunk 已被远端接受但 checkpoint 尚未落盘的崩溃窗口提供显式查询边界。backend 必须按 session、offset 和 immutable bytes 返回已接受的 chunk end。

## 产品接入形状

AsterDrive 一类产品后续按下面的方向组合，而不是让 Forge 读取产品 DTO：

```text
Explorer / Finder / Linux VFS
        ↓
Forge platform engine
        ↓
aster_forge_cloud_files_core
        ↓
product-owned backend adapter
        ↓
product API / credential / repository
```

产品 adapter 负责：

- remote ID 到 `CloudNamespaceId` / `CloudRootId` / `CloudItemId` 的转换；
- remote ETag/version 到 revision 的转换；
- backend capability 声明；
- Forge error classification 到产品 error 的映射；
- authentication、endpoint、DTO、权限和产品 root policy。
- 实现真实 `ChangeCheckpointStore` / `MutationJournalStore` / `ContentStorageStore` / `ContentCacheWriteStore` / `ContentUploadStore`，并把 Forge store error 映射到产品日志、metrics 和 lifecycle policy。
- 为 session 创建绑定正确 backend 的 `HydrationCoordinator`，并把 native request/cancel/completion 映射为 waiter lifecycle。
- 把 provider cache 删除、platform eviction 与 native/kernel invalidation 分别实现为可观察、幂等 physical effect。
- 把 temporary/sparse cache physical commit 与 coverage transaction 分开，并在 restart 时观察真实 physical state。
- 实现 immutable local snapshot reader、`CloudContentUploadBackend` 和 generation-aware `ContentUploadStore`，保留产品 operation identity、transport、认证、分片协议、status endpoint、retry/backoff 和 worker lifecycle。

最小调用方向为：

```text
DriveCloudBackend implements CloudFilesBackend
        ↓
Forge coordinator calls get/list/changes/read
        ↓
adapter maps Drive transport and DTOs
        ↓
Forge receives CloudItem/ChangePage/ContentReadResponse
```

## Error boundary

`CloudFilesCoreError` 表达 core value/contract validation：

- 空 identity/revision/cursor/digest；
- native identity 超过平台限制；
- alignment intersection overflow；
- 缺少 core 要求的 identity invariant；
- invalid item shape；
- invalid/overflowing byte range；
- content response revision/size/range contract violation。
- invalid mutation intent/state transition；
- zero session generation。
- invalid content-storage ownership/range/lease state；
- invalid eviction target/effect/state transition。
- invalid cache-write physical/coverage transition；
- invalid local generation、upload session/chunk/offset/outcome transition。

`CloudBackendError` 表达 product adapter 的通用失败分类：

- `NotFound`
- `AuthenticationRequired`
- `PermissionDenied`
- `Conflict`
- `PreconditionFailed`
- `InvalidRequest`
- `RateLimited`
- `TemporarilyUnavailable`
- `Unsupported`
- `InvalidResponse`
- `Internal`

retry 与 kind 分开建模：`RetryAdvice::Never`、`Retry` 或 `RetryAfter(Duration)`。产品 adapter 可以保留详细 transport/product error 到自己的日志，再只把稳定分类和 retry advice 交给 Forge。

`CloudFilesStoreError` 表达 durable store port 的稳定分类：

- `NotFound`
- `Conflict`
- `InvalidTransition`
- `PersistenceFailure`

store diagnostic context 面向 adapter 日志，不是产品用户文案。数据库 driver error、transaction retry、schema migration 和 connection lifecycle 由具体产品 store 实现保留并映射。

`HydrationError` 区分：

- `Cancelled`：单个 waiter 的 cancellation 赢得 terminal race；
- `Backend`：product-owned content backend 的结构化失败；
- `Contract`：返回 revision、size、offset、length 或 assembly coverage 违反 Forge contract。

`ContentUploadRunError` 与 `MutationRunError` 都区分：

- `Backend`：产品 transport adapter 失败；
- `Store`：durable store 或 immutable source 失败；
- `Contract`：backend outcome、source 或 durable transition 违反共享合同；
- `RecordNotFound`：scheduler 请求恢复的 operation 没有 durable record。

runner outcome unknown 不是 error；它使用 `RemoteOutcomePending` 明确交还产品 scheduler。generation takeover 也不是 store failure；它返回 `Fenced`，让旧 executor 停止并由新 generation 恢复。

platform adapter 负责把这些分类映射到 CFAPI failure completion、File Provider error 或 FUSE errno/reply lifecycle。

产品 API 层负责把这些错误映射成自己的状态码、错误 envelope、日志字段和用户文案。

## 测试要求

```bash
cargo test -p aster_forge_cloud_files_core
cargo clippy -p aster_forge_cloud_files_core --all-targets -- -D warnings
```

当前共有 230 个 contract tests，覆盖：

- 所有公开 opaque/string/byte value object 的 constructor、accessor、consume、`Display`/`Debug`、精确字节保留与各自 empty-field classification；
- scope collision isolation；
- identity/path 分离；
- Windows-style native identity limit；
- metadata/content revision 类型分离；
- revision/digest 分离；
- page/change/directory token 类型分离；
- capability 每个布尔、optional、limit、storage、eviction 与 cancellation 维度的 intersection；
- range alignment zero/identity/equal/multiple/coprime/LCM 与 overflow；
- `default()` unsupported baseline 与 `unconstrained()` intersection identity；
- optional unsupported capability；
- core identity invariant validation；
- root/file/directory item shape；
- root move rejection 与 same-root move identity stability；
- multi-page enumeration 无遗漏/重复；
- page cursor parent scope；
- anchored change paging、tombstone 和 idempotent replay；
- cursor-expired 与 identity-invalid reset；
- exact whole/range content reads；
- stale revision precondition failure；
- response size/revision/offset/range/EOF validation，以及 malformed backend response 传播；
- missing scope/item/parent/content、file-as-directory 和 malformed/non-UTF8/out-of-bound cursor classification；
- backend error kind 与 retry advice 分离；
- Full/Limited/ReadOnly synthetic capability profiles；
- provisional backend trait object safety。
- batch persist 后 active cursor 不前移；
- effects replayable 前禁止 cursor commit；
- batch/effects/cursor 每个 post-persist crash window；
- duplicate cursor commit、pending marker 与 committed batch replay 幂等；
- wrong pending batch、second pending batch、base cursor drift 和 batch identity reuse conflict；
- command intent post-persist response loss；
- observed mutation pre-journal startup rediscovery；
- remote commit/outcome marker gap 通过 `AlreadyCommitted` 恢复；
- explicit remote-outcome-unknown 阻止提前 platform reconciliation；
- old session generation completion fence；
- ordered session closing/draining/closed lifecycle；
- mutation value model、全部 session transition pair、非法 state transition 和 missing-record `NotFound`；
- content mutation 必须携带 exact-item immutable snapshot，non-content mutation 禁止携带 snapshot；
- completed mutation 不进入 recovery scan、不重复 remote effect；
- completed mutation 对相同 late durable marker 幂等，对不同 late outcome 报 conflict；
- `Committed` 与同 item 的 `AlreadyCommitted` 被视为同一个 durable effect，允许并发 idempotent apply 收敛；
- multi-scope mutation recovery 隔离并按 operation ID 稳定排序。
- generic mutation runner 覆盖 submit/resume、committed/already-committed/precondition outcome、delete 无 item、create/modify 返回 item 结构验证；
- mutation apply/reconcile backend failure 保留最后一个 durable retry point，lost remote return 使用同一 idempotent intent 重放；
- mutation intent/apply/outcome/platform/completion 每个 post-persist response-loss window 均可恢复；
- explicit unknown outcome 可多次返回 pending，known reconciliation 后继续完成且不形成忙循环；
- runner 拒绝宣称 transition 成功但 durable record 未前进的 store；旧 generation 在 remote apply 前或 remote effect 后的晚到 outcome write 都会被 fence，新 generation 可恢复同一 intent；
- 两个并发 mutation runner invocation 通过 idempotent backend 与 durable store transition 收敛到一个 completed record。
- exact concurrent hydration 只产生一个 backend read；
- concurrent whole-file hydration 只产生一个 whole read；
- overlapping/nested/subset range 复用 shared segment，只下载 uncovered gap；
- 一个 waiter 可复用多个 existing segment，并只补中间或两侧 gap；
- whole work 可服务 range waiter，expected size 不同的请求不共享；
- physical alignment 扩大 backend read，但 waiter 只收到 logical bytes；
- revision 或 session generation 不同的 work 相互隔离；
- backend revision drift 使 assembly 失败；
- 一个 waiter 取消后其他 waiter 继续；
- subset cancellation 只释放 unshared gap；
- 所有 waiter 取消后 backend future 被 drop；
- cancellation/completion 只有一个 terminal outcome；
- pre-poll cancellation、重复 cancellation、drop cancellation handle、drop unpolled waiter 与 coordinator 先 drop 的 cleanup；
- shared backend failure 只执行一次并传递给全部 waiter；
- wrong offset/size、short/long range、short whole 等 adversarial response 被拒绝；
- EOF empty range 仍执行 revision fence；
- beyond-EOF range 在 backend work 前被拒绝。
- rename 后 cache key 稳定、不同 content revision 隔离；
- sparse coverage overlap/adjacency merge、size boundary 和 empty-file completeness；
- platform/provider/hybrid ownership state 分离；
- pin、dirty、两个并发 cache write 与六类 runtime lease guard 的稳定顺序；
- hydration lease 与 eviction reservation 竞争、reservation 后 lease fence；
- provider/platform/hybrid eviction physical plan；
- provider metadata reconciliation 前必须完成 native invalidation；
- hybrid all eviction 必须观察三个独立 physical effect；
- intent/effect/metadata/completion 每个 post-persist response-loss window，以及 terminal state 后相同 marker replay；
- physical effect 后 marker 前退出通过 startup observation 恢复；
- completed eviction 从 recovery scan 排除，recoverable records 跨 scope 隔离并稳定排序；
- content/cache-write/upload/eviction/checkpoint store 的全部 missing-record transition 返回 `NotFound`。
- provider-cache physical commit 前禁止 coverage advance；
- 多个 cache-write reservation 独立计数，最后一个 completion 后才释放 eviction blocker；
- physical bytes 后 marker 前退出通过 startup observation 恢复；
- whole empty-file cache commit 不伪造 zero-length range；
- local dirty generation 单调推进并 fence stale observation/clear；
- upload chunk 绑定 exact session/local generation/offset/size；
- resumable accepted offset 单调推进，完整上传前禁止 remote commit；
- 多个 upload intent 获取独立 `Upload` lease，最后一个 completion 后才释放对应 blocker；
- remote commit unknown 通过 `AlreadyCommitted` reconciliation 恢复；
- committed upload outcome 必须返回当前 item metadata；
- old upload completion 不清除 newer local generation；
- precondition failure 保留 dirty snapshot；
- upload intent/checkpoint/commit/outcome/metadata/completion 每个 post-persist response-loss window；
- cache-write、upload 与 eviction recovery 都验证 scope filtering、stable ordering 和 completed exclusion。
- runner 按配置分片读取 exact immutable source，覆盖空文件、尾分片和 durable accepted-offset resume；
- runner 拒绝 partial/oversized source、越界 start offset、错误 session/ack，并保留最后一个 durable checkpoint；
- runner 拒绝宣称成功但 durable record 没有前进的 store transition，避免错误实现触发忙循环；
- source/store/backend failure、checkpoint lost return 和 remote-commit retry 都可从 durable state 继续；
- unknown commit 可多次返回 pending，再通过 reconcile 完成，期间不重复上传 chunk 或再次 commit；
- precondition outcome 完成 upload lease lifecycle，但保留 dirty snapshot 交给产品冲突策略；
- 较新 active `SessionGeneration` fence 旧 executor，并可以继续同一 immutable intent；
- 两个并发 runner invocation 通过 idempotent backend 与 monotonic store transition 收敛到一个 completed record。

这些测试固定公开行为合同和可恢复边界，不以把 defensive impossible branch 暴露成测试接口为目标。当前仍未由安全公共 API 到达的分支主要是 `usize`/ID/counter 理论溢出、mutex poison 恢复，以及 private constructor 对公共类型形状已经排除的防御检查；生产代码不会为覆盖率数字加入 test hook。

## 参考项目

- 当前 core contract 由 Windows CFAPI、Apple File Provider、Linux FUSE 平台合同和 synthetic fixtures 塑造。
- AsterDrive、AsterYggdrasil 和未来产品只作为 downstream adapter/integration validation，不参与定义 core 公共语义。
