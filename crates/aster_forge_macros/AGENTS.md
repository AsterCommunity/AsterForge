# aster_forge_macros

## 边界

- 只放产品无关的声明式代码生成机制。ACME 状态、验证方式及所有执行策略留在消费者仓库。
- 当前 `wire_enum!` 是 macro_rules 宏，不是 proc-macro；无生产依赖，不引入运行时或框架。
- 生成代码必须使用明确的 std/core/serde 路径，避免捕获调用方局部类型。
- 保留未知 wire 字符串，严格区分大小写；不得将未知值默认为成功或某个已知变体。
- 依赖由根 workspace 管理；入口先写 `//!` 文档，再写非测试 panic/unwrap 等 lint。
- 枚举分支使用穷尽 match。修改展开代码时更新 README 和 docs/crates/aster_forge_macros.md。

## 验证

- 运行 `cargo test -p aster_forge_macros --locked`，包括 doctest 和 compile-fail 示例。
- 运行 `cargo clippy -p aster_forge_macros --all-targets --locked -- -D warnings`。
- 覆盖已知及未知值、非字符串、特殊字符、大小写、调用方名称遮蔽和无效声明。
- 消费者的协议状态和验证策略须由消费者自己的测试验证。
