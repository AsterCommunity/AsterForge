# `aster_forge_cloud_files_linux`

`aster_forge_cloud_files_linux` 是 `aster_forge_cloud_files_core` 的 Linux FUSE 平台 adapter。当前 Phase 4 Batch 1 提供真实的**只读** `fuser` 闭环：恢复稳定 inode/generation record、paged directory snapshot、revision-bound range read、file/directory handle、FUSE reply/errno 映射，以及从 FUSE callback 到调用方 Tokio runtime 的有界非阻塞 dispatch。

它不是云盘 daemon、Linux service 或 AsterDrive client。产品仓库仍拥有远端 backend adapter、认证、权限、持久化 inode records、mount path、用户级 service、桌面集成、安装更新和用户可见错误。

## 适用边界

该 crate 当前适合验证或接入：

- `CloudItemKey <-> inode/generation` 的稳定恢复；
- `lookup`、`getattr`、`opendir`、`readdir`、`open`、`read`、`release` 和 `releasedir`；
- 由 `open` 捕获 content revision 后的 exact range read；
- 目录 handle 级 snapshot 和 FUSE directory cookie；
- callback 不等待 network/database work 的 bounded async handoff；
- read-only mount 的 `EROFS` 拒绝路径。

当前不包含：write/flush/release 的 mutation journal、writeback cache、`FUSE_INTERRUPT` 精确取消、kernel cache invalidation、`mmap`/大文件实机矩阵、daemon restart/stale mount recovery，或产品 daemon packaging。这些能力必须在对应的 durable mutation/cache/invalidation 生命周期完成后再加入，不能先用空 callback 占位。

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
printf 'x' > /tmp/aster-forge-memory-cloud/new.txt
```

最后一条应因 read-only mount 失败。example 采用两页 root enumeration，包含 `docs/guide.txt` 和 16 KiB `numbers.bin`，所以 `find`、nested lookup 和 range read 都会经过 native engine。它的 backend 只存在于运行进程内；从另一终端卸载：

```bash
fusermount3 -u /tmp/aster-forge-memory-cloud
```

系统没有 `fusermount3` 时可使用发行版对应的 FUSE unmount 命令。不要用 `kill -9` 代替正常 unmount；后续 stale-mount/restart 批次会专门覆盖异常退出恢复。

## 错误边界

`LinuxErrorCode` 是产品无关 backend/error 分类与 native errno 的中间层：

| 分类 | FUSE errno |
| --- | --- |
| item/inode 不存在 | `ENOENT` |
| 认证或权限失败 | `EACCES` |
| revision conflict 或 stale handle | `ESTALE` |
| 临时不可用、限流或 dispatch 饱和 | `EAGAIN` |
| 不支持的 backend operation | `ENOSYS` |
| 无效 native request | `EINVAL` |
| adapter contract/internal failure 或 session closing | `EIO` |

产品层决定日志、重试、认证 UI、冲突提示和 API 文案；Forge 不把这些产品语义写入 FUSE adapter。

## 测试要求

crate contract tests 覆盖 inode/root/duplicate/scope 边界、Linux filename 边界、分页 directory snapshot、rename 后 stable key、revision-bound range/EOF read、stale handle、missing persisted mapping 和 dispatcher saturation/closing。

2026-07-27 的只读 baseline 已通过 Rocky Linux 10.2 aarch64（kernel `6.12.0-211.22.1.el10_2.aarch64`）实机验证。example 由 macOS 使用 `cargo zigbuild` 交叉编译；VM 未安装 Rust toolchain。验证覆盖真实 `/dev/fuse` mount/unmount、多页和 nested enumeration、32 路并发 range read、16 路并发 mixed read、EOF、create/append/mkdir/unlink/chmod 的 `EROFS` 拒绝、正常卸载后的 provider 退出，以及无残留 FUSE mount。产品接入仍必须验证 durable inode store、正常 daemon restart 后恢复相同 inode record、daemon crash/stale mount、产品 service lifecycle 和远端 backend 故障。

```bash
cargo test -p aster_forge_cloud_files_linux --all-targets
cargo clippy -p aster_forge_cloud_files_linux --all-targets -- -D warnings
cargo check -p aster_forge_cloud_files_linux --target x86_64-unknown-linux-gnu
cargo zigbuild --release -p aster_forge_cloud_files_linux \
  --example linux_memory_cloud_drive --target aarch64-unknown-linux-gnu
```

## 参考

- [`aster_forge_cloud_files_core`](./aster_forge_cloud_files_core.md)
- [`fuser::Filesystem`](https://docs.rs/fuser/0.18.0/fuser/trait.Filesystem.html)
- [`fuser::ReplyData`](https://docs.rs/fuser/0.18.0/fuser/struct.ReplyData.html)
- [Linux FUSE documentation](https://www.kernel.org/doc/html/latest/filesystems/fuse/fuse.html)
