# aster_forge_utils

`aster_forge_utils` 收纳低依赖、产品无关的小工具。它不是杂物间，只有确实跨项目重复、且不属于更具体 crate 的 helper 才应该放进来。

## 适用场景

- boolean-like 字符串解析。
- Gravatar hash 和 URL 拼接。
- best-effort 临时文件和目录清理。
- HTML 和 inline script 占位符 escaping。
- HTTP date 和条件请求 ETag 比较。
- UUID 和 token 生成。
- 网络地址和 trusted proxy 解析。
- 安全数值转换。
- 路径渲染和运行时临时目录路径。
- RAII 临时文件/目录清理。
- UTF-8 安全截断和字符数量统计。
- URL 解析、origin/base URL 规范化。
- public-site origin 列表解析和 origin/path 拼接。

不适合放在这里的内容：

- 产品配置结构。
- 配置值 normalizer 和运行时默认值读取，应该用 `aster_forge_config`。
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

### avatar

主要 API：

- `gravatar_hash(email)`
- `gravatar_url(email, size, base_url)`

`gravatar_hash` 会 trim、lowercase 邮箱后计算 Gravatar 使用的 MD5 hex。`gravatar_url`
拼出 Aster 服务当前统一使用的公开 URL 形状：`{base}/{hash}?d=identicon&s={size}&r=g`。

Gravatar base URL 的配置写入校验和默认值回退在 `aster_forge_config` 中，别放回
`utils`。产品侧仍然负责用户头像来源策略、上传头像路由、缓存头、可用尺寸，以及是否启用
Gravatar。

### bool_like

`parse_bool_like(value)` 支持常见布尔字符串。适合环境变量和兼容配置读取。

### fs

主要 API：

- `cleanup_temp_file(path)`
- `cleanup_temp_dir(path)`
- `cleanup_runtime_temp_root(temp_root)`

这些 helper 面向临时文件和临时目录的 best-effort 清理：缺失文件/目录会被忽略，其他失败记录
warn 日志但不返回错误。`cleanup_temp_dir` 会对 `DirectoryNotEmpty` 做短暂重试，覆盖 macOS
Spotlight/Finder 或文件监听器在删除过程中短暂写入目录的情况。

不要把它们用于需要事务语义、用户可见错误或存储驱动一致性的删除操作；那些场景应该保留产品侧
显式错误处理。

### html

主要 API：

- `escape_html(value)`
- `escape_script_json(value)`

`escape_html` 用于把普通文本插入已经存在的 HTML text/attribute 占位符，例如后端渲染
`index.html` 里的标题、图标 URL、CSP meta 和 CSRF token 名称。它会转义 `&`、`"`、`'`、
`<`、`>`。

`escape_script_json` 用于 JSON 序列化之后、插入 inline `<script>` 之前的二次 escaping。
它会转义 HTML parser 相关字符和 JavaScript 行分隔符，避免 `</script>` 这类文本打断脚本块。

这两个 helper 不是富文本 sanitizer。用户提交的 HTML 是否允许、如何过滤标签和属性，仍然是产品
安全策略。

### id

主要 API：

- `new_uuid()`
- `new_short_token()`
- `UniqueUuidAttempt<T>`
- `UNIQUE_UUID_MAX_ATTEMPTS`

唯一 UUID 生成流程通过 `UniqueUuidAttempt` 把“候选冲突”和“成功结果”表达出来。产品侧决定冲突如何查询数据库。

### http_validators

主要 API：

- `format_http_date(time)`
- `parse_http_date(value)`
- `http_date_epoch_seconds(time)`
- `if_match_header_matches(raw, resource_exists, current_etag)`
- `if_none_match_header_matches(raw, resource_exists, current_etag)`

该模块实现 transport-neutral 的 HTTP conditional request 基础语义：`If-Match` 使用强
ETag 比较，`If-None-Match` 使用弱比较，二者都支持 `*`，并拒绝没有任何 entity tag
的空列表。实现不依赖 Actix/Axum；产品负责把 `HttpValidatorError` 映射为 REST、WebDAV、
WOPI 或其他协议所需的状态码和响应体。

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

### text

主要 API：

- `char_count(value)`
- `truncate_utf8_to_max_bytes(value, max_bytes)`

`char_count` 统计的是 Unicode scalar value，不是 grapheme cluster。它适合现有 Aster
服务里“最多 N 个 `chars()`”这种规则；如果产品要按用户感知字符处理，需要在产品侧另设
Unicode segmentation 策略。

`truncate_utf8_to_max_bytes` 用于保守的字节限制，例如文件名、任务展示名、外部错误摘要等。
它不会截断到非法 UTF-8 边界。

### url

主要 API：

- `parse_url`
- `parse_absolute_url`
- `has_http_scheme`
- `is_https_or_loopback_http`
- `normalize_http_base_url`
- `normalize_origin`
- `parse_public_site_origins`
- `normalize_public_site_origins_config_value`
- `runtime_public_site_origins_with`
- `public_site_origin_for_request`
- `join_origin_and_path`

适合 external auth callback、CORS origin、公开 base URL 等配置规范化。`public_site_*` helper 只处理产品无关的 origin 解析、去重、请求 origin 匹配和 URL 拼接；产品侧仍然保留具体 config key、runtime snapshot、日志上下文和错误映射。

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
