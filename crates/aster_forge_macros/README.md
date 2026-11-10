# aster_forge_macros

产品无关的声明式宏。当前提供 `wire_enum!`，为字符串协议枚举生成已知值映射、`Unknown(String)`、Serde 编解码及 owned String 转换。

不包含产品状态名、协议决策、验证方法或运行时；不是 proc-macro，无生产依赖。调用方直接依赖启用 `derive` 的 `serde`。

```rust
use aster_forge_macros::wire_enum;

wire_enum! {
    /// Product-owned status.
    pub enum Status {
        /// Ready to proceed.
        Ready => "ready",
    }
}

assert_eq!(Status::from("future".to_owned()), Status::Unknown("future".into()));
```

映射严格区分大小写；空字符串、Unicode 和其他未知值逐字保留。非字符串输入拒绝反序列化。`Unknown` 是保留变体名，至少声明一个已知变体，重复 wire 值编译失败。已知变体元数据用于文档，不支持对单个变体使用 `cfg` 或覆盖 Serde 表示；按条件启用时对整次宏调用使用 `cfg`。调用方仍须限制响应体大小、判断未知值能否执行，并映射产品错误。宏生成的 Serde 路径要求依赖名为 `serde`。

```sh
cargo test -p aster_forge_macros --locked
cargo clippy -p aster_forge_macros --all-targets --locked -- -D warnings
```

测试覆盖已知/未知字符串往返、大小写、空值、Unicode、长值、控制字符、非字符串拒绝、路径卫生和编译失败契约。完整接入说明见仓库的 `docs/crates/aster_forge_macros.md`。

License: MIT OR Apache-2.0.
