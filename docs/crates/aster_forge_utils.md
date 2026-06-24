# aster_forge_utils

`aster_forge_utils` 收纳低依赖、产品无关的小工具。它不是杂物间，只有确实跨项目重复、且不属于更具体 crate 的 helper 才应该放进来。

## 适用场景

- boolean-like 字符串解析。
- UUID 和 token 生成。
- 网络地址和 trusted proxy 解析。
- 安全数值转换。
- 路径渲染和运行时临时目录路径。
- RAII 临时文件/目录清理。
- URL 解析、origin/base URL 规范化。

不适合放在这里的内容：

- 产品配置结构。
- 文件名校验，应该用 `aster_forge_validation`。
- object key 校验，应该用 `aster_forge_storage_core`。
- API pagination，应该用 `aster_forge_api`。

## Cargo 接入

```toml
[dependencies]
aster_forge_utils = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## 模块

### bool_like

`parse_bool_like(value)` 支持常见布尔字符串。适合环境变量和兼容配置读取。

### id

主要 API：

- `new_uuid()`
- `new_short_token()`
- `UniqueUuidAttempt<T>`
- `UNIQUE_UUID_MAX_ATTEMPTS`

唯一 UUID 生成流程通过 `UniqueUuidAttempt` 把“候选冲突”和“成功结果”表达出来。产品侧决定冲突如何查询数据库。

### net

主要 API：

- `is_loopback_host(host)`
- `parse_trusted_proxies(values)`
- `is_trusted_proxy(ip, trusted)`
- `real_ip_from_forwarded_for(...)`

适合反向代理、真实客户端 IP 和 loopback http 判断。产品侧仍要决定 trusted proxy 配置来源。

### numbers

提供 `i64_to_usize`、`u64_to_i64`、`calc_total_chunks` 等检查转换。不要在产品里用裸 `as` 处理外部输入和数据库值，容易溢出或静默截断。

### paths

提供：

- `join_path`
- `normalize_path`
- `render_runtime_relative_path`
- `resolve_config_relative_path`
- `resolve_config_relative_sqlite_url`
- `temp_file_path`
- `runtime_temp_dir`
- `upload_temp_dir`
- `task_temp_dir`

这些函数处理的是运行时路径，不负责 object storage key 安全。

### raii

`TempFileGuard` 和 `TempDirGuard` 用于测试或临时流程失败时自动清理。长期资源生命周期不要靠 RAII guard 偷偷控制，产品服务应该显式管理。

### url

主要 API：

- `parse_url`
- `parse_absolute_url`
- `has_http_scheme`
- `is_https_or_loopback_http`
- `normalize_http_base_url`
- `normalize_origin`

适合 external auth callback、CORS origin、公开 base URL 等配置规范化。

## 错误边界

`UtilsError` 分为：

- `InvalidValue`
- `NumericConversion`

产品侧应在配置加载、API handler 或 service 边界映射成具体错误。不要把 `UtilsError` 直接作为产品 API error 类型。

## 测试要求

- 每个产品接入点覆盖非法输入。
- 数值转换测试要包含负数、超上限和边界值。
- URL/origin 测试要覆盖 loopback HTTP、HTTPS、wildcard。
- trusted proxy 测试要覆盖多代理链。

## 参考项目

- AsterDrive：URL、proxy、upload chunk、token、临时路径和数值转换场景丰富。
- AsterYggdrasil：适合看轻量项目如何直接调用 Forge utils，避免保留无意义 facade。
