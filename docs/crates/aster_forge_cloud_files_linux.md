# `aster_forge_cloud_files_linux`

`aster_forge_cloud_files_linux` 是 `aster_forge_cloud_files_core` 的 Linux FUSE 平台 adapter。Phase 4 Batch 1 提供真实的只读 `fuser` 闭环：恢复稳定 inode/generation record、paged directory snapshot、revision-bound range read、file/directory handle、FUSE reply/errno 映射，以及从 FUSE callback 到调用方 Tokio runtime 的有界非阻塞 dispatch。Batch 2A 增加现有文件的 direct-I/O writeback：完整 revision-bound hydration、调用方注入的 durable local staging、positioned write、truncate、flush、fsync、immutable dirty generation fence 和关闭后重新打开。Batch 2B 增加 mount `SessionGeneration` activation、dirty snapshot/size overlay 恢复、无需重复下载远端内容的 staged reopen，以及旧 mount session fence。Batch 2C 补齐 core runtime-neutral resumable upload runner 与 generation-aware durable transition，固定 Linux dirty snapshot 到产品 upload worker 的接入边界。Batch 2D 增加 regular-file durable create：产品事务分配稳定 item/inode/generation 与 mutation identity，Linux adapter 在同一 acceptance 中取得空 staging，并通过 `ReplyCreate` 一次返回 entry 与 direct-I/O handle。Batch 2E 增加 core generic `MutationRunner` 与 `CloudMutationBackend`，把 durable create 的 apply、remote-outcome-unknown reconciliation、产品/platform metadata reconciliation 和 completion 收成共享状态机。Batch 2F 把这些边界接入 memory example：`writeback.json` 原子保存 namespace/staging/mutation journal，独立 `remote.json` 模拟远端幂等 create ledger，示例产品 worker 负责 startup scan、apply/reconcile、generation fence 与 completion。

它不是云盘 daemon、Linux service 或 AsterDrive client。产品仓库仍拥有远端 backend adapter、认证、权限、持久化 inode records、mount path、用户级 service、桌面集成、安装更新和用户可见错误。

## 适用边界

该 crate 当前适合验证或接入：

- `CloudItemKey <-> inode/generation` 的稳定恢复；
- `lookup`、`getattr`、`opendir`、`readdir`、`open`、`read`、`release` 和 `releasedir`；
- 由 `open` 捕获 content revision 后的 exact range read；
- 目录 handle 级 snapshot 和 FUSE directory cookie；
- callback 不等待 network/database work 的 bounded async handoff；
- read-only mount 的 `EROFS` 拒绝路径。
- writable mount 中现有文件的 `O_WRONLY` / `O_RDWR`、positioned write、稀疏扩展补零与 truncate；
- 每次成功 write/truncate 在回复 FUSE 前返回已持久化的 `LocalContentSnapshot`；
- 可重复 flush、显式 fsync，以及关闭后从 dirty staging 重新打开。
- mount 激活时按 scope 恢复 dirty snapshots，并在第一次 open 前恢复 dirty size；
- daemon restart 后优先 reopen staged bytes，较低 `SessionGeneration` 的迟到写入由 store fence。
- product store 在同一 durable transaction 中保存 dirty snapshot 与 core upload intent，随后由调用方 executor 驱动 core upload runner。
- native `create` 通过产品注入的 `LinuxNamespaceMutationStore` 获得 durable stable identity、inode record、create intent 与空 staging session。
- create 成功后立即 write/read/fsync/reopen，并在 daemon restart 后恢复未完成的本地 create 与相同 inode/generation。

当前不包含：目录 create、rename/unlink/rmdir、kernel writeback cache、Linux engine 内建 upload/create worker、`FUSE_INTERRUPT` 精确取消、kernel cache invalidation、`mmap`/大文件实机矩阵、产品 metadata offline cache 或 daemon/service packaging。共享 upload/mutation runner 位于 core；Linux crate 不选择 operation ID、idempotency key、item ID、inode allocation policy、本地文件布局、数据库、backend transport 或 retry policy。Batch 2B 已证明 adapter 的 restart recovery contract，Batch 2C 已证明共享 upload runner 的 chunk/resume/reconcile/fence contract，Batch 2D 固定 native create 到产品原子 namespace transaction 的边界，Batch 2E 固定 create worker 到 generic mutation runner/store/backend 的边界，Batch 2F 只在 example 内提供 synthetic product implementation；产品接入仍要用自己的 crash-safe store、mount lifecycle、scheduler 和远端 adapter 完成同一验证。

## Cargo 集成

```toml
[dependencies]
aster_forge_cloud_files_core = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_cloud_files_core" }
aster_forge_cloud_files_linux = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_cloud_files_linux" }
tokio = { version = "1", features = ["rt-multi-thread"] }
```

Linux crate 的 native `fuser` module 只会在 `target_os = "linux"` 编译。inode table、engine、dispatch contract 可以在其他开发机运行测试；这不代表其他系统完成了 FUSE runtime 验证。

## 稳定 inode

`CloudItemKey` 是稳定、带 scope 的产品无关身份；inode 只是当前 Linux mount namespace 中的 native mapping。产品必须先从 durable store 恢复 `LinuxInodeRecord`，再把 mount 暴露给 kernel：

```rust
use std::sync::Arc;

use aster_forge_cloud_files_linux::{
    LINUX_ROOT_INODE, LinuxInode, LinuxInodeGeneration, LinuxInodeRecord, LinuxInodeTable,
};

let root_record = LinuxInodeRecord::new(root_key, LINUX_ROOT_INODE, LinuxInodeGeneration::new(1)?);
let child_record = LinuxInodeRecord::new(child_key, LinuxInode::new(2)?, LinuxInodeGeneration::new(1)?);
let table = Arc::new(LinuxInodeTable::new(root_record, [child_record])?);
```

table 不会从 pathname、filename 或 hash 推导 inode，也不会在 callback 中临时分配 inode。缺少某个 child record 时 engine 会拒绝暴露它，避免 rename、restart 或 stale handle 场景无声改变 native identity。产品 store 负责在第一次映射前原子保存 record，并在 daemon restart 前恢复同一 record。

## 只读 FUSE Engine

产品实现 core 的 `CloudFilesBackend`，提供 scoped metadata、paged `list_children` 和 revision-bound `read_content`。然后显式交入 product-owned Tokio runtime：

```rust
use aster_forge_cloud_files_linux::{
    LinuxAttributePolicy, LinuxReadOnlyEngine, LinuxReadOnlyFilesystem,
};

let attributes = LinuxAttributePolicy::new(
    1000,
    1000,
    0o444,
    0o555,
    std::time::Duration::from_secs(1),
)?;
let engine = LinuxReadOnlyEngine::new(backend, restored_inode_table, attributes);
let filesystem = LinuxReadOnlyFilesystem::new(engine, runtime.handle().clone(), 64)?;
```

`LinuxReadOnlyFilesystem` callback 只做 input validation 与 `LinuxRequestDispatcher::reserve()`；不能获得 permit 时立即返回 `EAGAIN`，closing 时返回 `EIO`。接受后的 task 在调用方 runtime 上执行 backend I/O，并独占一个 FUSE reply。panic 或 runtime shutdown 时未消费 reply 由 `fuser` 的 reply Drop 路径终结为 `EIO`，不会留下无终结请求。

`open` 拒绝非只读 access mode，mount 使用 `RO`、`DefaultPermissions`、`NoDev`、`NoSuid`、`NoExec`。成功 open 会捕获当前 `ContentRevision` 和 size；随后的 `read` 必须返回这个 revision 的 exact range bytes，revision drift 映射为 `ESTALE`，而不是混合不同版本的内容。

## Writable Existing-File Engine

`LinuxWritableEngine` 包装只读 engine，并要求调用方注入 `LinuxWritebackStore`。首次打开文件时会读取完整、revision-bound 的远端内容；store 可以从该内容建立 staging，也可以重新打开同一 item 已存在的 dirty staging。所有 writable mount 文件句柄都经过 store，因此文件关闭后再次只读打开仍会看到当前 dirty bytes，而不是旧远端 revision。

```rust
use std::sync::Arc;

use aster_forge_cloud_files_linux::{
    LinuxWritableEngine, LinuxWritableFilesystem, mount_writable,
};
use aster_forge_cloud_files_core::SessionGeneration;

let product_store = Arc::new(product_store);
let readonly = LinuxReadOnlyEngine::new(backend, restored_inode_table, attributes);
let engine = LinuxWritableEngine::activate_with_namespace(
    readonly,
    Arc::clone(&product_store),
    product_store,
    SessionGeneration::new(product_mount_generation)?,
).await?;
let filesystem = LinuxWritableFilesystem::new(engine, runtime.handle().clone(), 64)?;
mount_writable(filesystem, mountpoint, &mount_config)?;
```

`LinuxWritebackStore::activate_mount` 必须在返回前激活给定 `SessionGeneration`，fence 较低 mount generation，并返回同 scope 当前可恢复的 `LocalContentSnapshot`。engine 用 restored inode table 校验 snapshot identity 并恢复 size overlay；`open_recovered` 返回带相同 mount generation 和 exact snapshot 的 session。recovered open 仍验证 backend item 是普通文件，但跳过远端 content hydration。

`LinuxWritebackStore::write` 与 `truncate` 的成功结果包含 `LinuxWriteCommit`，其中的 `LocalContentSnapshot` 必须在方法返回前已经可恢复。新的 mutating commit 必须严格增加 item-local generation；重复 flush/fsync 可以返回同一 generation。`release` 每个 open 最终调用一次，但 Linux 不会把 release reply error 返回给触发 close 的应用，因此 durability 错误必须由 write、flush 或 fsync 暴露，不能推迟到 release。

首版 writable mount 对每个文件句柄返回 `FOPEN_DIRECT_IO`，不请求 kernel writeback cache。这样 write offset、write reply 与 staging durability 顺序保持显式；writeback-cache page ordering、append flags、`mmap` 和 delayed write 属于后续独立批次。

## Durable regular-file create

启用 create 时，产品 store 通常由同一个实现同时提供 `LinuxWritebackStore` 与 `LinuxNamespaceMutationStore`。两个 trait 保持分开，是因为 namespace identity transaction 和现有文件的 byte staging 不是同一个机制；生产实现仍应在同一个数据库或文件事务边界中组合它们。

`LinuxCreateFileRequest` 只包含 native request facts：已解析的 stable parent key、name、mode、umask、handle access 和 active `SessionGeneration`。它不包含待创建 item key、inode、operation ID 或 idempotency key。`LinuxNamespaceMutationStore::create_file` 在返回前原子保存并返回：

```text
product-allocated CloudItemKey
+ LinuxInodeRecord(inode + generation)
+ MutationIntent(DesiredMutation::Create)
+ empty local staging / LinuxWriteSession
+ active SessionGeneration comparison
-> one durable transaction
-> LinuxCreateFileAcceptance
-> ReplyCreate(entry + direct-I/O handle)
```

adapter 会验证：item 是指定 parent/name 下的空 regular file；item、inode record 与 staging session 使用相同 stable key；create intent 精确匹配 scope/parent/name/kind；fresh acceptance 使用当前 generation；key/inode/parent-name 不与当前 mount mapping 冲突。任一产品返回合同不一致都在 inode 对 kernel 可见前失败。

`activate_namespace` 返回同 scope 尚需本地呈现的 durable creates。较新的 mount generation 可以恢复旧 intent，但来自未来 generation 的 record 会被拒绝。`open_created_file` 负责在 remote create 尚未 materialize 时重新打开 clean empty staging；一旦产生 dirty snapshot，常规 `LinuxWritebackStore::open_recovered` 路径优先。目录 handle 仍是 snapshot：create 之前打开的 directory handle 不会中途出现新 entry，create 后新开的 handle 会包含 durable local overlay。

远端 create 由产品 worker 调用 core `MutationRunner::resume()` 推进；产品注入 `CloudMutationBackend` 与 `MutationJournalStore`，继续拥有 transport、认证、DTO、stable/provisional identity mapping、真实数据库事务、retry/backoff 调度，以及 create 完成后是否建立 content upload intent。runner 统一处理幂等 apply、remote-outcome-unknown reconciliation、generation fence、durable product/platform metadata reconciliation 与 completion。Linux callback 不调用产品 HTTP API；`LinuxCreatedFile` 中的 item/record/intent 只是产品原子事务已经完成的可验证结果。产品在 parent/name 已存在时返回 `AlreadyExists`，native 层映射为 `EEXIST`；generation fence 映射为 `ESTALE`，持久化或合同故障映射为 `EIO`。

## Resumable upload 接入

Linux engine 不在 `write`、`flush` 或 `release` callback 里直接调用远端 backend。产品 `LinuxWritebackStore` 应把一个 native write 的 durable acceptance 做成自己的原子事务：

```text
staged bytes + LocalContentSnapshot
+ caller-allocated ContentUploadIntent
+ active SessionGeneration check
-> one durable product transaction
-> FUSE write/truncate reply
-> product worker calls core ContentUploadRunner::resume
```

`ContentUploadIntent` 所需的 base revision 来自 `LinuxWriteOpenRequest::base_revision()`，source snapshot 来自 `LinuxWriteCommit::snapshot()`，执行 generation 来自当前 `LinuxWritableEngine::session_generation()`。`OperationId`、`IdempotencyKey` 和 `ContentLeaseId` 由产品 durable store 分配；Linux adapter 不推导这些值。

worker 使用 core 的 `LocalContentSnapshotReader` 读取 exact immutable generation，使用 `CloudContentUploadBackend` 映射产品 resumable transport，并把同一个 active generation 传给每次 `ContentUploadStore` transition。runner 返回：

- `Completed`：known outcome、metadata reconcile 与 upload lease release 已耐久；
- `RemoteOutcomePending`：commit 结果仍未知，产品按自己的 backoff 再次调度 `resume()`；
- `Fenced`：当前 worker generation 已过期，停止旧执行者，由新 mount generation 恢复同一 intent。

不要在 Linux engine 收到 `LinuxWriteCommit` 后再通过独立 observer 异步创建 intent。那会制造 snapshot 已成功回复 FUSE、但 crash 时 upload intent 仍不存在的窗口。原子性属于产品 store transaction，不属于 FUSE callback 或 core runner。

## 准备 FUSE 设备

运行预编译或交叉编译后的 example 二进制不需要在目标机安装 Rust，但目标 Linux kernel 必须提供 FUSE，并安装包含 `fusermount3` 的 FUSE 3 用户态工具。常见发行版可使用：

```bash
# Ubuntu / Debian
sudo apt-get update
sudo apt-get install -y fuse3

# Fedora / RHEL / Rocky Linux
sudo dnf install -y fuse3

# Arch Linux
sudo pacman -S --needed fuse3
```

安装后检查三件事：kernel module 可用、`/dev/fuse` 已暴露、当前用户对设备可读写。

```bash
sudo modprobe fuse
command -v fusermount3
ls -l /dev/fuse
test -r /dev/fuse && test -w /dev/fuse
```

如果 `/dev/fuse` 存在但当前用户没有读写权限，先检查设备所属组；仅在系统确实配置了 `fuse` 组时把用户加入该组，随后注销并重新登录：

```bash
stat -c '%A %U %G %t:%T' /dev/fuse
getent group fuse
sudo usermod -aG fuse "$USER"
```

如果 `modprobe fuse` 成功后仍不存在 `/dev/fuse`，问题通常位于虚拟机、容器或宿主机的设备暴露策略，需要在外层环境放行 FUSE device。不要把手工 `mknod` 或永久 `chmod 666` 当作默认修复；单独创建 device node 不能绕过缺失的 kernel module、device cgroup 或虚拟化限制。

## 内存云盘 Example

Linux VM 中确保 `/dev/fuse` 可用，并创建挂载目录。使用预编译二进制时直接执行：

```bash
mkdir -p /tmp/aster-forge-memory-cloud
./linux_memory_cloud_drive /tmp/aster-forge-memory-cloud
```

第二个可选参数启用 example-only 的跨进程 synthetic state，用于 restart/crash recovery 验收：

```bash
mkdir -p /tmp/aster-forge-memory-state
./linux_memory_cloud_drive \
  /tmp/aster-forge-memory-cloud \
  /tmp/aster-forge-memory-state
```

如果目标机已有源码和 Rust toolchain，也可以从 workspace 运行：

```bash
cargo run -p aster_forge_cloud_files_linux --example linux_memory_cloud_drive -- \
  /tmp/aster-forge-memory-cloud
```

另一终端测试真实 FUSE 路径：

```bash
find /tmp/aster-forge-memory-cloud -maxdepth 2 -type f -print
cat /tmp/aster-forge-memory-cloud/hello.txt
cat /tmp/aster-forge-memory-cloud/docs/guide.txt
dd if=/tmp/aster-forge-memory-cloud/numbers.bin bs=1 skip=4096 count=32 status=none | od -An -tx1
printf 'updated through FUSE\n' > /tmp/aster-forge-memory-cloud/hello.txt
cat /tmp/aster-forge-memory-cloud/hello.txt
truncate -s 8 /tmp/aster-forge-memory-cloud/readme.txt
dd if=/dev/zero of=/tmp/aster-forge-memory-cloud/numbers.bin bs=1 count=4 conv=fsync status=none
printf 'new file\n' > /tmp/aster-forge-memory-cloud/new.txt
cat /tmp/aster-forge-memory-cloud/new.txt
```

已有文件的 write、truncate、flush/fsync 和 reopen 应成功；regular-file create 也会分配 synthetic stable key、inode/generation、create intent 和空 staging，随后支持相同 writeback 路径。提供 state 目录时，`writeback.json` 在同一原子替换中保存 namespace record、staged bytes、dirty generation、active session 和 mutation journal；example product worker 通过 core `MutationRunner` 把 create 提交到独立 `remote.json`，再把 committed item metadata 与 reconciliation marker 同事务写回 `writeback.json`。卸载或 provider crash 后以更高 mount generation 重启，`new.txt` 应保留相同 synthetic inode identity 与内容，未完成 mutation 会使用相同 operation/idempotency identity 恢复。example 采用两页 root enumeration，包含 `docs/guide.txt` 和 16 KiB `numbers.bin`，所以 `find`、nested lookup、whole hydration、positioned write、create overlay 和 reopen 都会经过 native engine。静态 metadata/content fixture 与 synthetic remote mutation ledger 有意分开；该布局证明产品端可以满足 Forge transaction/recovery contract，不是 production store，也不代表 remote change feed、metadata offline cache 或 content upload 已接通。另一终端卸载：

```bash
fusermount3 -u /tmp/aster-forge-memory-cloud
```

系统没有 `fusermount3` 时可使用发行版对应的 FUSE unmount 命令。正常退出使用 unmount；故障恢复测试可设置 `ASTER_FORGE_CLOUD_FILES_EXAMPLE_REMOTE_COMMIT_PAUSE_MS`，让 backend 在 `remote.json` 已原子提交、outcome 尚未返回 worker 的窗口暂停，再终止 provider。清理 stale mount 后取消该变量并从同一 state 目录重启，worker 应从 `RemoteApplying` 使用同一 idempotency key 得到 `AlreadyCommitted`，随后完成本地 metadata reconciliation 和 journal completion。

## 错误边界

`LinuxErrorCode` 是产品无关 backend/error 分类与 native errno 的中间层：

| 分类 | FUSE errno |
| --- | --- |
| item/inode 不存在 | `ENOENT` |
| create parent/name 已存在 | `EEXIST` |
| 认证或权限失败 | `EACCES` |
| revision conflict 或 stale handle | `ESTALE` |
| 临时不可用、限流或 dispatch 饱和 | `EAGAIN` |
| 不支持的 backend operation | `ENOSYS` |
| 无效 native request | `EINVAL` |
| adapter contract/internal failure 或 session closing | `EIO` |

产品层决定日志、重试、认证 UI、冲突提示和 API 文案；Forge 不把这些产品语义写入 FUSE adapter。

## 测试要求

Linux crate 的 portable suite 有 40 个 tests，其中 9 个直接覆盖 existing-file writeback/recovery，11 个覆盖 durable create，另有 1 个 handle-exhaustion unit test。Linux-only example harness 再增加 4 个 synthetic product tests，覆盖 remote effect 已提交但 outcome marker 缺失后的 higher-generation restart convergence、旧 `writeback.json` 缺少 mutation vector 时从 durable create intent 恢复 `IntentPersisted`、`remote.json` 持久化失败时不得在内存中发布虚假 committed outcome，以及 session 进入 `Closing` 后同 generation 的迟到 native create 必须被 fenced。完整矩阵覆盖 inode/root/duplicate/scope 边界、Linux filename 边界、分页 directory snapshot、rename 后 stable key、revision-bound range/EOF read、stale handle、missing persisted mapping 和 dispatcher saturation/closing。writeback tests 额外覆盖完整 base hydration、read/write visibility、稀疏扩展、truncate、size overlay、重复 flush/fsync、同 generation 不同 immutable snapshot 冲突、access mode、partial store read、wrong item identity、generation regression、并发 commit 乱序 fence、empty/overflow write、restart dirty-size recovery、recovered-open hydration bypass、scope/unknown/root/conflicting recovery records、old mount fence 和 recovered session generation validation。create tests 覆盖 empty acceptance、stable key/inode/generation、立即 write/read/sync/reopen、directory snapshot、existing-name conflict、同名与不同名并发、invalid name/parent、store failure before exposure、product contract substitution、lost reply restart recovery、old mount fence 和未配置 namespace port。共享 upload/mutation runner 的完整、空文件、分片、恢复、unknown outcome、precondition、source/backend/store failure、lost return、并发执行和 generation takeover 由 core contract suite 覆盖。

2026-07-27 的只读 baseline 与 2026-07-28 的 writable/recovery baseline 均已通过 Rocky Linux 10.2 aarch64（kernel `6.12.0-211.22.1.el10_2.aarch64`）实机验证。example 由 macOS 使用 `cargo zigbuild` 交叉编译；VM 未安装 Rust toolchain。只读验证覆盖真实 `/dev/fuse` mount/unmount、多页和 nested enumeration、并发 range/mixed read、EOF 与只读 mutation 拒绝。writable 验证覆盖覆盖/追加后 reopen、truncate、稀疏 positioned write、`fsync`、16 个并发 handle 写不同 offset、namespace mutation 的 `ENOSYS`，以及正常 unmount/provider exit。recovery 验证覆盖 generation 1 写入后 provider crash、stale mount 清理、generation 2 恢复 dirty bytes/size、恢复后继续写、正常卸载，以及 generation 3 再次恢复最新内容。synthetic JSON state 仅用于证明 adapter contract，不代表 production durable store、metadata offline cache 或远端 upload 已完成。产品接入仍必须使用真实 durable inode/writeback store 验证 service lifecycle、backend 故障与 crash recovery。

2026-07-29 的 Batch 2D binary 继续在同一 Rocky Linux VM 实机通过 durable-create smoke：native create 后立即 write/read/sync，`O_EXCL` duplicate 返回失败，并发同名 create 只有一个成功，并发不同名 create 都成功；mkdir 与 rename 继续保持未实现；随后 provider 被强制终止、stale mount 被清理，generation 2 从同一 synthetic state 恢复相同 inode 与 exact bytes，继续 append/sync 后正常卸载。该验证证明 FUSE `ReplyCreate`、local namespace overlay、JSON atomic acceptance 和 mount-generation recovery 的组合，不代表远端 backend create/reconcile 已实现。

2026-07-29 的 Batch 2F binary 在更新后的 Rocky Linux 10.2 aarch64 kernel `6.12.0-211.39.1.el10_2.aarch64` 上通过 synthetic remote-create 故障窗口：generation 1 创建 `restart.txt` 后，`remote.json` 已有唯一 committed entry，而 `writeback.json` 仍停在 `remote_applying`；此时强制终止 provider，清理 stale mount，并取消 pause 以 generation 2 重启。重启后相同 inode `7` 与 20 字节 staged content 均保持，journal 收敛为 `completed` + `already_committed`，remote ledger 数量仍为 1，正常卸载后 session durable state 为 `closed`。4 个 Linux-only synthetic product harness tests 也在该 VM 直接执行通过。该验证仍是 synthetic product adapter，不替代真实产品 transport、数据库事务、change feed 或 content upload 验收。

```bash
cargo test -p aster_forge_cloud_files_linux --all-targets
cargo clippy -p aster_forge_cloud_files_linux --all-targets -- -D warnings
cargo check -p aster_forge_cloud_files_linux --target x86_64-unknown-linux-gnu
cargo zigbuild -p aster_forge_cloud_files_linux \
  --test memory_cloud_drive_example --target aarch64-unknown-linux-gnu
cargo zigbuild --release -p aster_forge_cloud_files_linux \
  --example linux_memory_cloud_drive --target aarch64-unknown-linux-gnu
```

## 参考

- [`aster_forge_cloud_files_core`](./aster_forge_cloud_files_core.md)
- [`fuser::Filesystem`](https://docs.rs/fuser/0.18.0/fuser/trait.Filesystem.html)
- [`fuser::ReplyData`](https://docs.rs/fuser/0.18.0/fuser/struct.ReplyData.html)
- [Linux FUSE documentation](https://www.kernel.org/doc/html/latest/filesystems/fuse/fuse.html)
