# 接入原则

这页定义 Forge crate 接入到产品仓库时应遵守的边界。它比单个 crate 页面更重要，因为后续抽模块的时候主要靠这些规则防止共享库变成一锅粥。

## 1. 共享机制，不共享业务状态

Forge 可以拥有通用状态机的机械部分，例如：

- 任务 lease 是否仍然有效。
- 重试间隔如何按尝试次数增长。
- 缓存后端如何读写字节。
- 数据库连接如何重试。
- 分页 cursor 如何解析。

Forge 不应该拥有产品状态，例如：

- 文件是否属于某个团队。
- 用户是否有管理员权限。
- 某个后台任务代表哪种业务流程。
- 外部认证账号如何绑定到本地用户。
- 某个存储策略是否允许去重或远程节点。

## 2. 错误在产品边界映射

Forge 的错误类型用于表达共享机制失败，例如 `TaskCoreError`、`DbError`、`ExternalAuthError`。产品仓库应该在 service 边界把它们转换成产品错误。

不要让 API handler 直接依赖 Forge 错误文案。文案、状态码、审计字段和本地化属于产品层。

## 3. 不做无意义薄封装

如果产品仓库里有这种函数：

```rust
fn sanitize_storage_prefix(prefix: &str) -> Result<String> {
    normalize_storage_prefix(prefix)
}
```

它没有增加语义，只是在制造历史包袱。接入 Forge 时应该直接调用 Forge API，除非薄 facade 真的承担了产品边界职责，例如：

- 映射错误类型。
- 注入产品配置。
- 记录产品指标。
- 加入审计上下文。

## 4. trait 适配显式写在产品侧

对于 `aster_forge_tasks`、`aster_forge_cache`、`aster_forge_external_auth` 这类 crate，Forge 提供 trait 和机械流程，产品侧实现 trait，把数据库 repository、metrics、runtime config 接进去。

这种显式适配比隐藏全局状态更啰嗦，但长期更稳：测试可以替换 store，Drive 和 Yggdrasil 也可以保持不同业务规则。

## 5. 测试覆盖跟风险走

接入只替换纯函数工具时，单元测试覆盖输入输出即可。

接入运行时、数据库、任务、外部认证这类模块时，至少要覆盖：

- 成功路径。
- 失败路径。
- 重试或降级路径。
- 并发或 token fence。
- shutdown / cancellation。

## 6. 参考项目不是复制理由

Drive 和 Yggdrasil 是参考，不是标准答案。Drive 通常功能更全，Yggdrasil 通常边界更轻。抽到 Forge 前要判断逻辑是否真的是产品无关的共享机制。
