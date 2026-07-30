# `aster_forge_cloud_files_linux`

`aster_forge_cloud_files_linux` 是 `aster_forge_cloud_files_core` 的 Linux FUSE 平台 adapter。它提供只读 enumeration/range read、有界 callback dispatch、direct-I/O durable writeback、resumable content upload 接入、regular-file/directory create、rename/move、unlink/rmdir、generation-fenced crash recovery，以及 durable remote overlay 到 kernel invalidation 的平台边界。共享 `MutationRunner` 和 `ContentUploadRunner` 位于 core；Linux crate 负责把已经由产品事务接受的结果映射成稳定 inode、directory snapshot、FUSE reply 和 native notification。

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
- `mkdir`、same/cross-parent `rename`、`RENAME_NOREPLACE`、file replacement、`unlink`、empty `rmdir` 与 `ENOTEMPTY`/`EISDIR`/`ENOTDIR` 映射；
- 每个 namespace acceptance 在 kernel exposure 前已经持久化完整 core mutation intent，重启后保持相同 inode/generation；
- `LinuxRemoteEntry`/`LinuxRemoteChange` 把产品已提交的 remote upsert/delete 恢复到 engine overlay；
- `spawn_mount_read_only`/`spawn_mount_writable` 返回 background session，`LinuxKernelNotifier` 应用 engine 产生的 entry/inode/delete invalidation plan。

当前不包含：kernel writeback cache、`mmap`/大文件实机矩阵、Linux engine 内建 transport/worker、产品 metadata 数据库、change cursor policy、冲突策略、retry/backoff、daemon/service packaging 或桌面 UX。`fuser 0.18` dispatcher 对 `FUSE_INTERRUPT` 返回 `ENOSYS`，因此当前没有 request-unique 级精确 native cancellation；产品仍可使用 core hydration/upload/mutation cancellation 和 generation fence。Linux crate 不选择 operation/idempotency/item/inode identity，不持有 endpoint、认证、DTO 或产品错误文案。

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

## Namespace mutation 与 remote change

`LinuxNamespaceMutationStore` 的目录 create、rename 和 remove 方法与 regular-file create 遵守同一原则：产品事务先保存 stable item/inode/generation、完整 `MutationIntent`、旧位置 tombstone、replacement facts 和 active `SessionGeneration`，engine 校验 acceptance 后才更新 FUSE 可见 overlay。`RENAME_EXCHANGE` 与 `RENAME_WHITEOUT` 当前明确返回 `ENOSYS`；普通 rename 和 `RENAME_NOREPLACE` 已接入。

远端 change feed 仍由产品拥有。正确顺序是：

```text
backend change + product conflict policy
-> product metadata/inode transaction commits
-> LinuxRemoteChange with exact old/replaced location
-> LinuxWritableEngine::apply_remote_change(...).await
-> ordered LinuxInvalidation plan
-> LinuxKernelNotifier::apply_all
-> advance product-owned change cursor
```

新 mount 应使用 `activate_with_namespace_and_remote`，在 dirty snapshot 恢复前注入产品持久化的 `LinuxRemoteEntry` records。运行期间的 upsert/delete 使用异步 `apply_remote_change(...).await`：engine 会先解析真实 old/destination entry，再在同一个 overlay state 临界区内重新校验并更新 key/inode/parent-name/tombstone 索引。它拒绝 scope 漂移、stable key 对应的 inode/generation 替换、inode reuse、错误的 previous location、遗漏或不匹配的 replacement，以及与未完成本地 namespace mutation 的碰撞。冲突如何解决仍由产品事务决定。

需要 kernel notification 时使用 `spawn_mount_writable` 或 `spawn_mount_read_only`。返回的 `LinuxBackgroundSession` 提供 `notifier()`、`join()` 和 `unmount_and_join()`；blocking `mount_writable`/`mount_read_only` 继续适合不需要外部 change worker 的简单 host。`LinuxKernelNotifier` 不进入 core，也不参与产品数据库事务。

## Interrupt 边界

`fuser 0.18` 能解析 `FUSE_INTERRUPT`，但其高层 dispatcher 当前直接回复 `ENOSYS`，`Filesystem` trait 也没有可实现的 interrupt callback。因此 Linux adapter 没有伪造 request-unique cancellation。已接受的 FUSE task 仍由 bounded dispatcher 保证一次终结；daemon shutdown、generation takeover、hydration、upload 和 mutation cancellation 使用现有显式 handle/fence。升级 `fuser` 后只有在高层 API 暴露 request unique 与 interrupt callback，并完成 race/late-reply 实机矩阵，才能声明 native interrupt 支持。

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

已有文件的 write、truncate、flush/fsync 和 reopen 应成功；regular-file create 也会分配 synthetic stable key、inode/generation、create intent 和空 staging，随后支持相同 writeback 路径。提供 state 目录时，`writeback.json` 在同一原子替换中保存 namespace record、staged bytes、dirty generation、active session 和 mutation journal；example product worker 通过 core `MutationRunner` 把 create 提交到独立 `remote.json`，再把 committed item metadata 与 reconciliation marker 同事务写回 `writeback.json`。staged file 与对应 immutable generation 使用共享 `Arc<[u8]>`，write 只复制被修改文件，不复制完整 store；JSON 使用紧凑编码，文件写入在多线程 Tokio runtime 中进入 blocking region，completed mutation/upload 只保留有界诊断尾部并回收不再引用的 immutable generations。卸载或 provider crash 后以更高 mount generation 重启，`new.txt` 应保留相同 synthetic inode identity 与内容，未完成 mutation 会使用相同 operation/idempotency identity 恢复。example 采用两页 root enumeration，包含 `docs/guide.txt` 和 16 KiB `numbers.bin`，所以 `find`、nested lookup、whole hydration、positioned write、create overlay 和 reopen 都会经过 native engine。静态 metadata/content fixture 与 synthetic remote mutation ledger 有意分开；该布局证明产品端可以满足 Forge transaction/recovery contract，不是 production store，也不代表 remote change feed、metadata offline cache 或 content upload 已接通。另一终端卸载：

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

Linux crate 的 portable suite 当前有 40 个 tests：existing-file writeback/recovery 9 个、durable create 11 个，并覆盖 backend/inode/value/directory/dispatcher 合同。Linux-only memory example harness 有 23 个 tests，额外覆盖 exact immutable upload、terminal history/immutable generation 上限、bounded cursor recovery pages、lost remote return、legacy JSON recovery、generation fence、mkdir/nested mkdir、same/cross-parent rename、`RENAME_NOREPLACE`、replacement kind errors、unlink/empty/non-empty rmdir、delete 后同名 file/directory 重建、rename 到旧 tombstone、旧 directory handle snapshot、持久化失败不发布、namespace journal restart、remote create/rename/delete 幂等收敛、stale previous/replacement 拒绝、remote overlay invalidation plan，以及 nested create 在 rename 与迟到 upload 后的 reload。共享 upload/mutation runner 的空文件、分片、resume、unknown outcome、precondition、source/backend/store failure、并发执行和 generation takeover 由 core contract suite 覆盖。

2026-07-27 的只读 baseline 与 2026-07-28 的 writable/recovery baseline 均已通过 Rocky Linux 10.2 aarch64（kernel `6.12.0-211.22.1.el10_2.aarch64`）实机验证。example 由 macOS 使用 `cargo zigbuild` 交叉编译；VM 未安装 Rust toolchain。只读验证覆盖真实 `/dev/fuse` mount/unmount、多页和 nested enumeration、并发 range/mixed read、EOF 与只读 mutation 拒绝。writable 验证覆盖覆盖/追加后 reopen、truncate、稀疏 positioned write、`fsync`、16 个并发 handle 写不同 offset、namespace mutation 的 `ENOSYS`，以及正常 unmount/provider exit。recovery 验证覆盖 generation 1 写入后 provider crash、stale mount 清理、generation 2 恢复 dirty bytes/size、恢复后继续写、正常卸载，以及 generation 3 再次恢复最新内容。synthetic JSON state 仅用于证明 adapter contract，不代表 production durable store、metadata offline cache 或远端 upload 已完成。产品接入仍必须使用真实 durable inode/writeback store 验证 service lifecycle、backend 故障与 crash recovery。

2026-07-29 的 Batch 2D binary 继续在同一 Rocky Linux VM 实机通过 durable-create smoke：native create 后立即 write/read/sync，`O_EXCL` duplicate 返回失败，并发同名 create 只有一个成功，并发不同名 create 都成功；mkdir 与 rename 继续保持未实现；随后 provider 被强制终止、stale mount 被清理，generation 2 从同一 synthetic state 恢复相同 inode 与 exact bytes，继续 append/sync 后正常卸载。该验证证明 FUSE `ReplyCreate`、local namespace overlay、JSON atomic acceptance 和 mount-generation recovery 的组合，不代表远端 backend create/reconcile 已实现。

2026-07-29 的 Batch 2F binary 在更新后的 Rocky Linux 10.2 aarch64 kernel `6.12.0-211.39.1.el10_2.aarch64` 上通过 synthetic remote-create 故障窗口：generation 1 创建 `restart.txt` 后，`remote.json` 已有唯一 committed entry，而 `writeback.json` 仍停在 `remote_applying`；此时强制终止 provider，清理 stale mount，并取消 pause 以 generation 2 重启。重启后相同 inode `7` 与 20 字节 staged content 均保持，journal 收敛为 `completed` + `already_committed`，remote ledger 数量仍为 1，正常卸载后 session durable state 为 `closed`。4 个 Linux-only synthetic product harness tests 也在该 VM 直接执行通过。该验证仍是 synthetic product adapter，不替代真实产品 transport、数据库事务、change feed 或 content upload 验收。

2026-07-29 的 namespace/upload 收尾继续在该 VM 通过 23 个 Linux-only harness tests 与真实 `/dev/fuse` 矩阵：mkdir、nested mkdir、create/write/`fsync`、same/cross-parent rename、file/directory inode 在 rename 前后保持、non-empty rmdir 返回 `ENOTEMPTY`、unlink/empty rmdir，以及 provider `SIGKILL` 后清理 stale mount 并以更高 generation 重启。嵌套目录 inode `7`、文件 inode `8` 和 exact content 在重启后保持；迟到 upload metadata 不再改变原 create parent/name；重启后的 unlink + rmdir 也能清理 name index。remote overlay/notifier 的 engine plan 已在 VM contract test 执行，真实产品 change feed 与 cursor transaction 仍由下游接入测试负责。

同日的一致性修复继续以公钥登录该 VM 验证：真实 `/dev/fuse` 上执行 file delete/recreate、directory remove/recreate、rename 到已删除 destination 后，`lookup`、read、stat 和新开的 `readdir` snapshot 均看到同一当前 entry。remote overlay 同时新增 stale previous、遗漏 replacement 与错误 replacement kind 的原子拒绝测试。

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
