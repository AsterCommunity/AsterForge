# `aster_forge_cloud_files_macos_bridge`

`aster_forge_cloud_files_macos_bridge` 是 `aster_forge_cloud_files_core` 与 Apple File Provider Swift extension 之间的产品无关 adapter。Phase 3 Batch 1 建立 Rust portable foundation：persistent item identifier、Apple system container、`NSFileProviderItemVersion` 双 component、owned item/page/content、只读 backend engine、extension generation/session fence，以及最小 C ABI allocation/handle ownership。Batch 2 增加 Swift shell、完整 XCTest 边界测试，以及 CMake 生成的 synthetic host app + File Provider extension fixture。Batch 3 增加 opaque sync anchor、updated/deleted change batch、`moreComing`、change enumeration cancellation 与 expired-anchor 映射。Batch 4 增加 materialized directory tracking、原子文件 store、refresh coalescing 与 working-set signal bridge。Batch 5 增加可选开发签名系统验收；Batch 6 把日常 E2E 收敛为可独立运行的 `macos_memory_cloud_drive` example。

它不是 AsterDrive macOS client。Swift shell 负责 `NSFileProvider*` 对象、completion handler、`Progress`、temporary content URL 和 framework callback；synthetic fixture 只验证产品无关的工程拓扑与内存目录。产品仓库仍负责远端 backend、认证、domain/account 映射、生产 store 选择与路径注入、真实 bundle ID、App Group、签名、安装更新和用户可见错误。

## 当前能力

- `CloudItemKey` 到 versioned、base64url persistent identifier 的无路径映射；
- root、working set、trash 三类 Apple system container 的独立分类；
- metadata/content revision 到 `NSFileProviderItemVersion` 的独立映射；
- 每个 version component 的 128-byte File Provider 限制；
- root item 与 root child 的 `NSFileProviderRootContainerItemIdentifier` 映射；
- paged enumeration 的 parent/scope、duplicate identifier、duplicate filename 校验；
- requested content revision 绑定的 whole-file fetch；
- extension generation、closing、draining、closed 和非 clone request lease；
- Swift-owned opaque session/request handles；
- owned byte buffer 与单次 release；
- null、UTF-8、NUL、panic 和 backend error 的稳定分类；
- Swift read-only item/error/version/capability mapping；
- item lookup、whole-file fetch、opaque page enumeration 和数据源提供的 opaque sync anchor；
- updated/deleted change batch、`moreComing`、500-byte anchor limit 和 expired-anchor reset；
- materialized directory 的 anchor-before-list、paged enumeration、anchored changes reconciliation；
- materialized refresh coalescing、cancellation/token fence 和迟到结果抑制；
- 可替换的 materialized store contract 与原子 JSON file implementation；
- replicated extension working-set `signalEnumerator` bridge；
- `Progress` cancellation、exactly-once terminal completion 和 request lease release；
- File Provider 同 volume temporary directory 中的原子 content staging；
- CMake Xcode generator 生成 host app、embedded `.appex`、Info.plist 和 entitlement；
- synthetic memory catalog：`README.txt`、`Documents/`、`Documents/hello.txt`；
- 可独立运行的 `macos_memory_cloud_drive` example，覆盖 Swift/Rust C ABI error value、session/request ownership、identifier encoding、目录枚举、内容读取、change feed 和 materialized working set。

item 与 enumeration page 转换必须显式提供当前 domain 的 exact `root_key`。bridge 用它同时校验 scope/root role，并把 root item 及其直接子项的 parent 映射为 Apple root system identifier；不会根据文件名、路径或产品 DTO 猜测根目录。

standalone example 是日常主测试路径，不依赖签名、provisioning 或 Finder domain。fixture host 仍提供 domain add/remove、Finder root 打开与 working-set signal 入口，extension 实现只读 item/fetch/enumerator 和 `materializedItemsDidChange`，用于编译检查与可选 Apple 原生验收。memory data source 能从 initial anchor 返回确定性的 updated batch、从 current anchor 返回空 terminal batch，并把未知 anchor 映射为 expired。开发签名运行时，materialized directory snapshot 会写入注入的 App Group 目录，working set 只保留已 materialized directory 本身及其直接子项。它仍不代表 durable remote change journal、自动 remote-push signal 或产品 packaging。

## Cargo 集成

```toml
[dependencies]
aster_forge_cloud_files_core = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_cloud_files_core" }
aster_forge_cloud_files_macos_bridge = { git = "https://github.com/AsterCommunity/AsterForge", package = "aster_forge_cloud_files_macos_bridge" }
```

crate 同时输出 Rust `rlib` 和可链接到 Swift target 的 `staticlib`。C header 位于：

```text
crates/aster_forge_cloud_files/macos_bridge/include/aster_forge_cloud_files_macos_bridge.h
```

## Identifier 边界

File Provider identifier 是 adapter mapping，不是路径或产品 DTO。普通 item 使用 `MacosFileProviderIdentifier::encode`；Apple root/working-set/trash 使用各自系统 identifier。system container 不会被解码成 `CloudItemKey`。当前 envelope 是可逆编码，不是加密或脱敏；调用方必须使用不含账号、路径、邮箱、token 等敏感信息的 opaque IDs。若产品现有 identity 本身敏感，正式接入必须使用 durable random token 到 `CloudItemKey` 的间接 mapping，不能把敏感原值交给 File Provider identifier。

```rust
use aster_forge_cloud_files_macos_bridge::MacosFileProviderIdentifier;

let identifier = MacosFileProviderIdentifier::encode(&item_key);
let restored = MacosFileProviderIdentifier::parse(identifier.as_str())?;
assert_eq!(restored.item_key()?, &item_key);
```

## Item Version

Apple 要求 `NSFileProviderItemVersion` 同时包含 content 和 metadata component，且每个 component 不超过 128 bytes。文件分别使用 Core 的 `ContentRevision` 与 `MetadataRevision`；目录使用稳定的 directory-content component，并保留真实 metadata revision。Swift 直接用两个 opaque byte slice 构造 `NSFileProviderItemVersion`，不得把二者合并或当成 digest。

## Swift C ABI

Swift 只能把 owned UTF-8/bytes 和 opaque handles 交给 Rust；Rust 不保存 Objective-C object、completion block 或 `NSProgress` pointer。主要 ownership 入口包括：

```text
identifier_encode / identifier_decode / buffer_release
session_create / session_begin_request
session_begin_closing / session_mark_disconnected
request_release / session_release
```

每个成功 request handle 必须在 completion、cancellation 或 terminal error 的唯一分支调用一次 `request_release`。session 先 closing，再 disconnected，最后在所有 request handle 释放后调用 `session_release`。重复释放非空 handle/buffer 属于 ABI 调用方错误。

当前 PoC 采用拓扑 A：File Provider extension 进程内静态链接 Rust bridge，通过窄 C ABI 获取 session、request lease 和 identifier mechanics。Forge 不同时保留一条未经验证的 helper/service IPC 路径；以后若引入跨进程拓扑，必须单独定义认证、sandbox、启动和恢复合同。

## Swift shell 与 fixture

Swift package 位于：

```text
crates/aster_forge_cloud_files/macos_bridge/swift
```

它不依赖产品 DTO 或 API，数据源通过 `MacosCloudFilesDataSource` 注入。SwiftPM、Xcode 和本地签名生成物由各自目录内的 `.gitignore` 管理，不污染仓库根规则。

`MacosCloudFilesDataSource` 同时提供当前 `MacosSyncAnchor` 和 anchored change enumeration。anchor 是最多 500 bytes 的 opaque token；Forge 不解析 cursor 内容，也不从时间戳、文件名或产品 revision 推导它。`MacosChangeBatch` 分开携带 updated snapshots 与 deleted identifiers，禁止重复 identifier 以及同一 batch 内的 update/delete 冲突。每个 change request 与 item/page/fetch 一样获取 Rust session lease，并在完成、错误或 enumerator invalidation 时 exactly-once release。

`MacosFileProviderMaterializedItemsReader` 按 Apple 要求先捕获 current anchor，再从空 page 开始完整分页，最后枚举 captured anchor 之后的所有 change batch，直到 `moreComing == false`。它只保存 materialized directory identifier；change 中同 identifier 变为普通文件时会移除，delete 同样移除。`MacosMaterializedSetTracker` 会合并连续 refresh 通知，extension invalidation 会取消 active enumerator，token fence 会阻止迟到 success 覆盖 store。`MacosFileMaterializedSetStore` 是可替换 contract 的原子文件实现；App Group ID、目录位置和生产数据库仍由产品注入。replicated extension 只 signal working set，`MacosWorkingSetSignaler` 不伪造 parent-container signal。

## Standalone 内存云盘 Example

`macos_memory_cloud_drive` 与 Windows/Linux memory example 使用同一原则：数据全部来自进程内 synthetic backend，Forge 只验证产品无关 adapter。macOS 版本不伪造 Finder mount，而是把 `MacosReadOnlyFileProviderRuntime`、native `NSFileProviderItem` mapping、enumerator/change observers、temporary content staging、Rust session/request C ABI、memory data source 和 working-set store 编译成普通 Mach-O executable。

生成和编译：

```bash
cmake \
  -S crates/aster_forge_cloud_files/macos_bridge/swift-fixture \
  -B /tmp/aster-forge-macos-fixture \
  -G Xcode \
  -DASTER_FORGE_CODE_SIGNING_ALLOWED=NO
cmake --build /tmp/aster-forge-macos-fixture --config Debug \
  --target AsterForgeCloudFilesMacosMemoryCloudDrive
```

生成的 binary 位于：

```text
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive
```

可直接验证根目录、嵌套目录、内容、change feed 和 materialized working set：

```bash
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive smoke
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive list root
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive list Documents
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive cat README.txt
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive cat Documents/hello.txt
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive changes initial
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive changes current
/tmp/aster-forge-macos-fixture/Debug/macos_memory_cloud_drive working-set
```

`smoke` 会依次验证 C/Swift error discriminant、Rust session/request ownership、Rust identifier encoding、runtime item lookup、root/nested native enumeration、temporary staged content 的 exact bytes、initial/current/expired native change enumeration 和 materialized working-set filtering。命令解析拒绝 absolute path、空 path component、`.`/`..` traversal、未知 anchor、目录 `cat`、文件 `list` 和缺失 item。

可签名 fixture 位于：

```text
crates/aster_forge_cloud_files/macos_bridge/swift-fixture
```

从 `LocalSigning.cmake.example` 创建本地 `LocalSigning.cmake`，填入 Team ID、host/extension bundle ID 和 App Group。该文件被局部 `.gitignore` 忽略。然后运行：

```bash
crates/aster_forge_cloud_files/macos_bridge/swift-fixture/scripts/build.sh
```

可选 Apple 原生系统验收使用：

```bash
crates/aster_forge_cloud_files/macos_bridge/swift-fixture/scripts/run-system-test.sh
```

脚本先验证本地签名身份、Host/Extension 签名和 Team ID，再注册 embedded extension。probe phase 验证显式 AppKit entry、参数解析、Swift async runner、报告编码和进程退出；baseline phase 随后枚举 Finder root、下载并核对两个内存文件、等待 materialized store、signal working set，并驱逐 `README.txt`。Host 退出后脚本终止或确认系统已回收 extension，再使用 `/bin/cat` 在 Host 未运行时重新下载文件。recovery phase 验证内容和 App Group store 可重开，最后移除 domain 和 plugin registration。该 runner 只负责最终 Apple 系统行为，不进入日常 CI 的通过条件。

本机 ad-hoc 签名可以执行 probe，也可能完成 extension discovery、domain 查询和 domain 注册；它缺少 Team ID 与 App Group provisioning，因此不等同于 File Provider 同步闭环。实测 provider 会保持 disabled，扩展请求在 grace period 后结束；移除 App Group entitlement 又会使系统直接拒绝该 domain。完整 baseline/recovery 以匹配 Team ID、bundle ID、App Group entitlement 和 provisioning profile 的开发签名为准。

无签名 CI 构建显式传入 `-DASTER_FORGE_CODE_SIGNING_ALLOWED=NO`。Rust staticlib 的 `MACOSX_DEPLOYMENT_TARGET` 与 Swift/Xcode target 保持一致；普通 Cargo build 不调用 Swift、CMake 或 Xcode，因此 crate 不使用 `build.rs` 编排平台工程。

## 错误边界

`MacosErrorCode` 是 Swift 映射 Apple error domain/code 前的产品无关分类：not found、not authenticated、permission denied、version out of date、try again、not supported、invalid argument、sync anchor expired、cancelled、provider not found 和 internal。认证与权限必须分开，Swift 才能分别映射 `NSFileProviderErrorNotAuthenticated` 与 `NSFileReadNoPermissionError`；产品层决定日志、重试、登录 UI 和冲突呈现。

## 测试要求

```bash
cargo test -p aster_forge_cloud_files_macos_bridge --all-targets
cargo clippy -p aster_forge_cloud_files_macos_bridge --all-targets -- -D warnings
MIRIFLAGS='-Zmiri-strict-provenance' \
  cargo +nightly-2026-04-12 miri test --locked -p aster_forge_cloud_files_macos_bridge
swift test --disable-sandbox \
  --package-path crates/aster_forge_cloud_files/macos_bridge/swift
cmake \
  -S crates/aster_forge_cloud_files/macos_bridge/swift-fixture \
  -B /tmp/aster-forge-macos-fixture \
  -G Xcode \
  -DASTER_FORGE_CODE_SIGNING_ALLOWED=NO
cmake --build /tmp/aster-forge-macos-fixture --config Debug \
  --target AsterForgeCloudFilesFixtureHost AsterForgeCloudFilesMacosMemoryCloudDrive
ctest --test-dir /tmp/aster-forge-macos-fixture -C Debug --output-on-failure
```

当前 deterministic tests 覆盖 identifier/system container/version limit、root mapping、paged enumeration、失败 page 的原子状态、backend identity/parent/revision/whole-file extent drift、完整 backend error mapping、session lifecycle、FFI null/zero/UTF-8/NUL/malformed input、buffer 与 opaque handle ownership。Swift XCTest 额外覆盖 1/128/129-byte version、499/500/501-byte page 与 sync anchor、精确 Apple error mapping、同步完成后取消、并发 terminal winner、session rejection、临时文件失败与迟到清理、opaque page/change anchor round trip、updated/deleted/more-coming change batch、expired anchor、enumerator invalidate 取消和 lease release。materialized tests 额外覆盖 store reopen/corruption、initial empty state、paged list + anchored changes、目录变文件、delete、`moreComing`、missing anchor、取消迟到回调、refresh coalescing、持久化失败、invalidate 后迟到 success fence 和 working-set signal error。example support tests 覆盖 command/default/help、root alias、nested path、argument cardinality、unknown command/anchor、absolute/traversal/empty component、list/change/smoke stable output；Swift XCTest 当前共 43 个。CTest 以 11 个进程级 case 运行 standalone binary，覆盖 smoke、root/nested list、nested cat、initial changes、working set，以及 traversal、missing item、list-file、cat-directory 和 expired-anchor failure。开发签名 runner 留作 Finder enumeration、system hydration/eviction、App Group reopen、extension termination 和 hostless hydration 的可选最终验收。

## 参考

- [`aster_forge_cloud_files_core`](./aster_forge_cloud_files_core.md)
- [Replicated File Provider extension](https://developer.apple.com/documentation/fileprovider/replicated-file-provider-extension)
- [Synchronizing the File Provider extension](https://developer.apple.com/documentation/fileprovider/synchronizing-the-file-provider-extension)
- [Defining File Provider content](https://developer.apple.com/documentation/fileprovider/defining-your-file-provider-s-content)
