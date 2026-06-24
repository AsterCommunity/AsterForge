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

## 核心 API

类型和常量：

- `FileCategory`
- `FileClassification`
- `FileClassificationError`
- `MAX_EXTENSION_LEN`
- `MAX_EXTENSION_FILTERS`

函数：

- `classify_file(name, mime_type)`
- `normalize_extension_filter(raw)`
- `parse_extension_filters(raw)`
- `parse_file_category(raw)`
- `extension_from_name(name)`
- `compound_extension_from_name(name)`

## 接入示例

```rust
let classification = aster_forge_file_classification::classify_file(
    &file.name,
    file.mime_type.as_deref().unwrap_or(""),
);
```

分类结果适合作为 API response 的派生字段。不要把它当成安全边界：用户可以上传伪造 MIME type 或奇怪扩展名，真正的处理器仍然要做自己的格式验证。

## 扩展名过滤

`parse_extension_filters()` 适合用于管理端配置或搜索参数。它会：

- 去掉空白。
- 统一大小写。
- 限制单个扩展名长度。
- 限制过滤器数量。

产品侧仍然应该决定非法过滤器对应什么 HTTP 状态码和文案。

## 测试要求

- 常见文件名和 MIME type 分类。
- 复合扩展名，例如 `.tar.gz`。
- 非法扩展名过滤器。
- 产品使用分类结果的 API response 快照或序列化测试。

## 参考项目

- AsterDrive：文件列表、搜索、预览和上传相关分类。
- AsterYggdrasil：如果只需要轻量文件名分类，可以直接使用纯函数，不引入业务 service。
