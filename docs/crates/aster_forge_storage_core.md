# aster_forge_storage_core

`aster_forge_storage_core` 提供存储相关的产品无关基础工具：安全 object key 处理和 S3 兼容配置规范化。

它不定义存储 driver trait，也不持有 credential、policy、bucket 绑定或业务文件实体。

## 适用场景

- 规范化相对 object key。
- 拼接和剥离 key prefix。
- 校验 key 不逃逸存储根。
- 规范化 S3-compatible endpoint 与 bucket。
- 统一 COS、MinIO、R2、AWS S3 等接入前的配置解析。

不适合放在这里的内容：

- 文件表、blob 表和 policy 表。
- 存储驱动生命周期。
- credential 加密。
- 远程节点同步。
- 文件上传业务流程。

## Cargo 接入

```toml
[dependencies]
aster_forge_storage_core = { git = "https://github.com/AsterCommunity/AsterForge" }
```

当前没有 feature flag。

## Object key

模块：`object_key`

主要 API：

- `normalize_relative_key(value)`
- `normalize_object_key(value)`
- `normalize_object_prefix(value)`
- `join_key_prefix(prefix, key)`
- `strip_key_prefix(prefix, key)`

`normalize_relative_key()` 是底层相对路径 normalizer。它会把空值和 root-like 输入表示为 `"."`，并拒绝上级目录逃逸。

产品侧做具体对象操作时优先用更明确的 helper：

- `normalize_object_key()`：用于 get/put/delete/metadata 这类具体对象操作；会拒绝空值和 root-like 输入。
- `normalize_object_prefix()`：用于 list/prefix scope；空值和 root-like 输入会映射成空 prefix。

产品侧应该把 `StorageCoreError::InvalidObjectKey` 映射为配置错误或 bad request，取决于输入来源。

接入注意点：

- object key 是存储内部路径，不等于用户可见文件名。
- 用户文件名校验请用 `aster_forge_validation::filename`。
- 不要手写 `format!("{prefix}/{key}")`，prefix 为空、斜杠重复和逃逸检查很容易漏。

## S3 config

模块：`s3_config`

主要 API：

- `normalize_s3_endpoint_and_bucket(endpoint, bucket)`
- `NormalizedS3Config`
- `S3ConfigError`

这个 helper 处理 S3-compatible 服务的基础连接字段：

- endpoint 为空时允许使用 provider 默认端点，但 bucket 仍然必填。
- endpoint 必须是 `http://` 或 `https://`，并且必须包含 host。
- endpoint 会去掉尾部 `/`，并拒绝 query string / fragment。
- bucket 会 trim，空值返回 `S3ConfigError::MissingBucket`。

产品侧仍然负责：

- access key / secret key 来源。
- region 默认值。
- path-style / virtual-host style。
- TLS 和代理配置。
- driver 初始化和健康检查。

## 测试要求

- key 正常化和逃逸拒绝。
- prefix join/strip 在空 prefix、有尾斜杠时稳定。
- S3 endpoint + bucket 的 path-style / virtual-host-style 场景。
- 产品驱动接入时覆盖真实配置样例，例如 MinIO、R2、COS。

## 参考项目

- AsterDrive：对象存储策略、S3/MinIO/R2/COS 接入和存储迁移任务。
- AsterYggdrasil：材质存储 key 和对象存储配置可作为轻量参考。
