# aster_forge_validation

`aster_forge_validation` 提供跨项目共享的输入校验。当前覆盖 display text、public asset URL、email、email policy list 和文件/文件夹名。

它的错误保持为简单消息，方便产品 API、service 和本地化层自行映射。

## 适用场景

- 邮箱格式校验和规范化。
- 提取邮箱域名。
- 邮箱 allowlist / blocklist 条目的规范化、排序去重和精确匹配。
- 运行时展示文本 trim、长度限制和控制字符处理。
- favicon、wordmark、provider icon 等公开前端资源 URL 的基础校验。
- 文件名 Unicode 规范化。
- 文件/文件夹名非法字符和保留名检查。
- copy name 生成。
- blob key 到存储路径转换。
- UTF-8 安全截断。

不适合放在这里的内容：

- 产品用户名规则。
- 密码策略。
- 产品 branding 默认值、legacy 文案迁移和公开配置 visibility。
- 产品账号是否允许注册、是否启用 allowlist、命中 blocklist 后返回什么错误码。
- object storage key 安全。
- API 错误码和本地化。

## Cargo 接入

```toml
[dependencies]
aster_forge_validation = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## Display

模块：`display`

主要 API：

- `normalize_bounded_display_text(field_name, value, max_len)`：trim 后检查最大长度和控制字符。空字符串合法，方便产品用空值表达“回退默认值”。
- `strip_control_chars(value)`：移除控制字符，适合 runtime 读取时对旧脏数据做 fail-soft 处理。
- `display_text_or_default(value, default, field_name, max_len)`：读取 runtime 展示文本，非法或空值时返回产品默认值。
- `normalize_public_asset_url(field_name, value, max_len)`：trim、检查最大长度、拒绝空白字符，并要求值是 leading-slash path 或 `http(s)` 绝对 URL。
- `is_public_asset_url(value)`：公开资源 URL predicate。
- `public_asset_url_or_default(value, default)`：读取 runtime 资源 URL，非法或空值时返回产品默认值。

这些 helper 只处理可复用的字符串规则。产品侧仍然负责：

- 配置 key，例如 `branding_favicon_url`。
- 默认值，例如 `/favicon.svg` 或产品自己的 wordmark。
- 是否对匿名用户公开。
- HTML placeholder 名称和前端注入位置。
- 是否要迁移早期模板文案。

## Email

模块：`email`

主要 API：

- `validate_email(email)`
- `normalize_email(email)`
- `email_domain(email)`

`normalize_email()` 会 trim 并做基本规范化。产品侧仍然要决定：

- 是否允许某些域名。
- 邮箱是否必须唯一。
- 邮箱变更是否需要验证。

## Email Policy

模块：`email_policy`

主要 API：

- `EmailPolicyList::from_items(items)`：严格解析条目，适合配置写入 normalizer。
- `EmailPolicyList::from_items_lossy(items, on_invalid)`：跳过非法条目并回调，适合运行时读取时 fail-open。
- `EmailPolicyList::matches(email, domain)`：检查规范化后的邮箱或域名是否精确命中。
- `normalize_email_policy_items(items)`：规范化、去重并按稳定顺序输出配置值。
- `parse_email_policy_item(item)`：把单条输入分类为 exact email 或 exact domain。
- `normalize_email_policy_email(email)`
- `normalize_email_policy_domain(domain)`
- `normalized_email_and_domain(email)`

条目规则：

- `alice@example.com` 表示精确邮箱。
- `example.com` 表示精确域名。
- `@example.com` 也表示精确域名。
- 空白条目会被忽略。
- 域名必须是 ASCII、包含点号，不能以点号开头/结尾，不能包含连续点号。
- 域名匹配是精确匹配，`example.com` 不会匹配 `sub.example.com`。

产品侧仍然负责：

- 配置 key 和默认值。
- allowlist 为空时是允许所有人还是拒绝所有人。
- allowlist / blocklist 的优先级。
- 命中策略后的 API 错误码和审计。

典型配置 normalizer：

```rust
let raw_items = aster_forge_config::parse_string_array_config_value(value, key)?;
let normalized = aster_forge_validation::email_policy::normalize_email_policy_items(raw_items)?;
let stored = serde_json::to_string(&normalized)?;
```

## Filename

模块：`filename`

主要 API：

- `normalize_name(name)`
- `normalize_validate_name(name)`
- `validate_name(name)`
- `storage_path_from_blob_key(blob_key)`
- `copy_name_template(name)`
- `format_copy_name(template, copy_number)`
- `format_copy_name_with_limit(template, copy_number, max_len)`
- `truncate_utf8_to_max_bytes(value, max_len)`
- `next_copy_name(name)`

文件名校验适合用户可见名称，不等同于对象存储 key 校验。存储 key 请用 `aster_forge_storage_core`。

## Copy name

`copy_name_template()` 和 `next_copy_name()` 用于复制文件时生成可读名称，例如：

- `report.pdf`
- `report (1).pdf`
- `report (2).pdf`

编号到 `u32::MAX` 耗尽时不会溢出，也不会回落到几乎必然已存在的 `(1)`：完整 stem 保留，
在其上开启新一层副本序列（`file (4294967295).txt` → `file (4294967295) (1).txt`），
一次性调用的产品侧也不会拿到一个大概率撞车的候选名。

产品侧仍然负责在数据库里检查最终名称是否冲突。Forge 只生成候选名，不查询产品 repository。

## 错误边界

`ValidationError` 只保存 message。产品侧应该根据输入来源映射：

- API 参数错误。
- 配置错误。
- 表单字段错误。

不要把所有 validation failure 都当 internal error。

## 测试要求

- email trim、大小写、非法格式。
- display text trim、长度限制、控制字符。
- public asset URL 的空值、root path、http(s)、非法 scheme 和 whitespace。
- 文件名空值、保留名、非法字符、长度限制。
- copy name 对带扩展名、无扩展名、多字节字符的处理。
- 产品侧唯一性检查要有独立测试。

## 参考项目

- AsterDrive：文件名、复制文件、上传路径、邮箱字段和 branding 配置。
- AsterYggdrasil：账号邮箱、用户输入校验和 branding 配置。
