# aster_forge_file_classification

`aster_forge_file_classification` 提供文件分类和扩展名过滤解析。它用于把文件名和 MIME type 归类成通用类别，方便产品做图标、预览策略、筛选和搜索。

## 适用场景

- 根据文件名和 MIME type 判断 `FileCategory`。
- 解析扩展名过滤器。
- 规范化扩展名。
- 提取普通扩展名和复合扩展名。

不适合放在这里的内容：

- 产品预览权限。
- 文件处理队列。
- 存储策略选择。
- 用户可见的文件图标组件。

## Cargo feature

```toml
[dependencies]
aster_forge_file_classification = { git = "https://github.com/AsterCommunity/AsterForge" }
```

可选 feature：

- `openapi`：给分类类型派生 OpenAPI schema。
- `sea-orm`：让 `FileCategory` 直接实现 SeaORM `ActiveEnum`，使用稳定的小写字符串值持久化。

## 核心 API

类型和常量：

- `FileCategory`
- `FileClassification`
- `FileClassificationError`
- `MAX_EXTENSION_LEN`
- `MAX_EXTENSION_FILTERS`
- `FILE_CLASSIFICATION_STORAGE_LEN`

函数：

- `classify_file(name, mime_type)`
- `normalize_extension_filter(raw)`
- `parse_extension_filters(raw)`
- `parse_file_category(raw)`
- `extension_from_name(name)`（只接受 ASCII 字母数字候选；`"dir.ext/file"`、`"report.pn g"` 这类路径样或含空白/标点的输入返回 `None`，因为提取值可能入库和展示）
- `compound_extension_from_name(name)`

MIME 回退分类里 spreadsheet/csv 分支先于通用 `text/` 分支，未知扩展名的 `text/csv` 会正确归类为 Spreadsheet 而非 Document。

## 接入示例

```rust
let classification = aster_forge_file_classification::classify_file(
    &file.name,
    file.mime_type.as_deref().unwrap_or(""),
);
```

分类结果适合作为 API response 的派生字段。不要把它当成安全边界：用户可以上传伪造 MIME type 或奇怪扩展名，真正的处理器仍然要做自己的格式验证。

扩展名冲突的裁决：`.ts` 归类为 Code（TypeScript 源码在存储产品里远多于 MPEG-TS 视频流）；真正的 MPEG transport stream 保留 `.m2ts` 扩展名和 `video/*` MIME 回退两条识别路径。

## 扩展名过滤

`parse_extension_filters()` 适合用于管理端配置或搜索参数。它会：

- 去掉空白。
- 统一大小写。
- 限制单个扩展名长度。
- 限制过滤器数量。

产品侧仍然应该决定非法过滤器对应什么 HTTP 状态码和文案。

## SeaORM 接入

需要把分类持久化到产品表时，直接启用 `sea-orm` feature：

```toml
aster_forge_file_classification = { git = "https://github.com/AsterCommunity/AsterForge", features = ["sea-orm"] }
```

启用后，`FileCategory` 可以直接作为 SeaORM entity 字段类型。产品仓库不应该再定义同名枚举或编写分类值转换层；产品 migration 仍然由产品仓库维护。

持久化 `extension`、`compound_extension` 和 `FileCategory` 时，字符串列宽至少使用
`FILE_CLASSIFICATION_STORAGE_LEN`。当前值为 `32`，产品 migration 可以直接引用该常量：

```rust
use aster_forge_file_classification::FILE_CLASSIFICATION_STORAGE_LEN;

ColumnDef::new(Files::Extension)
    .string_len(FILE_CLASSIFICATION_STORAGE_LEN)
    .not_null()
    .default("");
ColumnDef::new(Files::CompoundExtension)
    .string_len(FILE_CLASSIFICATION_STORAGE_LEN)
    .null();
ColumnDef::new(Files::FileCategory)
    .string_len(FILE_CLASSIFICATION_STORAGE_LEN)
    .not_null()
    .default(aster_forge_file_classification::FileCategory::Other.as_str());
```

`classify_file()` 保证返回的普通扩展名和复合扩展名不会超过该宽度；超长后缀不会写入
`extension`，分类会回退到 MIME type。以后如果 Forge 提高这个常量，已经落库的产品必须先
增加扩列 migration，再升级分类 crate，不能只更新依赖。

## 测试要求

- 常见文件名和 MIME type 分类。
- 复合扩展名，例如 `.tar.gz`。
- 非法扩展名过滤器。
- 产品使用分类结果的 API response 快照或序列化测试。

## 参考项目

- AsterDrive：文件列表、搜索、预览和上传相关分类。
- AsterYggdrasil：如果只需要轻量文件名分类，可以直接使用纯函数，不引入业务 service。
