# aster_forge_macros

## 用途边界

`wire_enum!` 生成字符串协议枚举及其 Serde/owned String 转换。调用方声明已知变体及 wire 值；宏自动增加 `Unknown(String)`，不决定未知值是否可执行或成功。不包含 ACME、HTTP、数据库或产品业务状态机，也不需要 runtime component 注册。

适合需要向前兼容的协议状态及扩展类型。要求遇到未知值立即拒绝的封闭配置枚举，直接使用普通 enum 与 Serde derive 更合适。

## Cargo feature 与接入

没有 feature 和生产依赖。调用方必须直接依赖名为 `serde` 且开启 `derive` 的 Serde。产品 workspace 统一维护依赖版本，成员只继承：

```toml
[workspace.dependencies]
aster_forge_macros = { git = "https://github.com/AsterCommunity/AsterForge" }
serde = { version = "1", features = ["derive"] }

# 成员 Cargo.toml:
# [dependencies]
# aster_forge_macros.workspace = true
# serde.workspace = true
```

生产接入固定已经发布到远端的 Forge revision；本地 path 联调不能作为已发布依赖的证据。

## 最小示例

```rust
use aster_forge_macros::wire_enum;

wire_enum! {
    /// Product-defined wire status.
    pub enum Status {
        /// Ready to proceed.
        Ready => "ready",
        /// Work in progress.
        Processing => "processing",
    }
}

assert_eq!(Status::from("ready".to_owned()), Status::Ready);
assert_eq!(Status::from("future".to_owned()), Status::Unknown("future".into()));
```

生成 Debug、Clone、PartialEq、Eq、Hash、Serialize、Deserialize，以及两个方向的 `From<String>` / `From<Enum> for String`。匹配严格区分大小写，不 trim、不改写未知字符串。非字符串输入返回 Serde 类型错误，不接受 enum 外部标签对象表示。

## 声明与错误边界

至少需要一个已知变体。`Unknown` 是保留名称，wire 值重复会产生编译错误。已知变体元数据用于文档，不支持单变体 cfg 或自定义 Serde 表示；条件开关作用于整个宏调用。生成代码用明确的 std/core/serde 路径，避免调用方类型同名导致捕获。

未知字符串不代表可信、可执行或成功。响应体大小限制、协议状态判断、错误呈现及日志脱敏由调用方负责。手动构造 `Unknown("ready".into())` 序列化后再读取会得到已知变体 Ready；无损保证针对 wire 字符串，不保证人为构造的非规范 enum 变体身份。

## 测试要求

- 已知值及未知值的 String/JSON 往返；大小写、空值、Unicode、控制字符及长字符串。
- 拒绝 null、数字、布尔、对象和数组；不静默转换类型。
- 调用方遮蔽 String/From 时依然可展开。
- 重复 wire 值、保留名称和空声明的 compile-fail 测试。
- 通过消费者测试验证实际协议枚举的已知值与执行策略。

```sh
cargo test -p aster_forge_macros --locked
cargo clippy -p aster_forge_macros --all-targets --locked -- -D warnings
```

## 参考项目

AsterACME 是首个本地消费者：core 声明资源状态，上层声明 challenge wire 类型；方法选择和状态机仍由 AsterACME 实现。尚无生产部署验收记录。
