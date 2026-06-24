# aster_forge_validation

`aster_forge_validation` 提供跨项目共享的输入校验。当前覆盖 email 和文件/文件夹名。

它的错误保持为简单消息，方便产品 API、service 和本地化层自行映射。

## 适用场景

- 邮箱格式校验和规范化。
- 提取邮箱域名。
- 文件名 Unicode 规范化。
- 文件/文件夹名非法字符和保留名检查。
- copy name 生成。
- blob key 到存储路径转换。
- UTF-8 安全截断。

不适合放在这里的内容：

- 产品用户名规则。
- 密码策略。
- object storage key 安全。
- API 错误码和本地化。

## Cargo 接入

```toml
[dependencies]
aster_forge_validation = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

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

产品侧仍然负责在数据库里检查最终名称是否冲突。Forge 只生成候选名，不查询产品 repository。

## 错误边界

`ValidationError` 只保存 message。产品侧应该根据输入来源映射：

- API 参数错误。
- 配置错误。
- 表单字段错误。

不要把所有 validation failure 都当 internal error。

## 测试要求

- email trim、大小写、非法格式。
- 文件名空值、保留名、非法字符、长度限制。
- copy name 对带扩展名、无扩展名、多字节字符的处理。
- 产品侧唯一性检查要有独立测试。

## 参考项目

- AsterDrive：文件名、复制文件、上传路径和邮箱字段。
- AsterYggdrasil：账号邮箱和用户输入校验。
