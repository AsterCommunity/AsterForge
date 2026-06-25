# aster_forge_external_auth

`aster_forge_external_auth` 提供外部认证 provider 的共享驱动、注册表、配置解析和协议类型。它负责 OAuth2/OIDC 连接器机制，不负责产品内的账号绑定、用户创建或权限策略。

## 适用场景

- OIDC 通用 provider。
- OAuth2 通用 provider。
- GitHub、Google、Microsoft、QQ 等专用 provider。
- 统一 provider registry。
- 授权 URL 生成、callback 交换、profile 规范化。
- provider 配置测试。

不适合放在这里的内容：

- 本地用户如何创建。
- 外部账号如何绑定/解绑。
- 登录审计。
- session 写入。
- 管理员配置页面。

## Cargo feature

默认 feature：

- `oidc`
- `oauth2`

可选 feature：

- `github`
- `google`
- `microsoft`
- `qq`
- `openapi`

示例：

```toml
aster_forge_external_auth = {
  git = "https://github.com/AsterCommunity/AsterForge",
  features = ["github", "google", "microsoft", "qq", "openapi"]
}
```

专用 provider 被放在 feature 后面，目的是让产品按需启用，避免默认二进制带上不需要的连接器。

## 核心类型

驱动层：

- `ExternalAuthProviderDriver`
- `ExternalAuthProviderDescriptor`
- `ExternalAuthProviderConfig`
- `ExternalAuthAuthorizationStart`
- `ExternalAuthCallback`
- `ExternalAuthProfile`
- `ExternalAuthProviderTestCheck`
- `ExternalAuthProviderTestResult`

注册层：

- `ExternalAuthProviderRegistry`
- `default_registry()`

类型层：

- `ExternalAuthProviderKind`
- `ExternalAuthProtocol`
- `ExternalAuthProviderOptions`
- `MicrosoftExternalAuthProviderOptions`

规范化层：

- `normalize::normalize_provider_key(value)`
- `normalize::normalize_required_field(value, field, max_len)`
- `normalize::normalize_optional_claim(value, field)`
- `normalize::normalize_scopes_with_default(value, default_scopes, protocol)`
- `normalize::normalize_scopes(value, protocol)`
- `normalize::normalize_icon_url_input(value, max_len)`
- `normalize::normalize_issuer_url_input(value, required, max_len)`
- `normalize::normalize_manual_endpoint_input(value, field, required, supported, max_len)`
- `normalize::normalize_allowed_domains(value)`
- `normalize::parse_allowed_domains(raw)`
- `normalize::email_domain_allowed(raw, email)`
- `normalize::state_hash(state)`
- `normalize::token_hash(token)`
- `normalize::normalize_return_path(value, max_len)`
- `normalize::normalize_flow_token(value, max_len)`

错误层：

- `ExternalAuthError`
- `MapExternalAuthErr`

## 最小接入流程

1. 产品从数据库或配置文件读取 provider 配置。
2. 转成 `ExternalAuthProviderConfig`。
3. 从 registry 取对应 driver。
4. 调 `start_authorization()` 生成跳转 URL 和 state/code verifier 等临时数据。
5. callback 时调 driver 完成 token exchange 和 profile 读取。
6. 产品 service 用 profile 查找或绑定本地用户。

Forge 到第 5 步结束。第 6 步必须留在产品仓库。

## 规范化规则

模块：`aster_forge_external_auth::normalize`

这个模块收纳 Drive / Yggdrasil 已经重复的 provider 配置规范化规则。它们属于外部认证协议或管理端表单的机械边界，不依赖产品数据库：

```rust
use aster_forge_external_auth::{ExternalAuthProtocol, normalize};

let key = normalize::normalize_provider_key(" GitHub ")?;
let scopes = normalize::normalize_scopes(Some("email profile"), ExternalAuthProtocol::Oidc)?;
let icon_url = normalize::normalize_icon_url_input(Some("/assets/github.svg".to_string()), 2048)?;
let return_path = normalize::normalize_return_path(Some("/settings?tab=login"), 2048)?;

assert_eq!(key, "github");
assert_eq!(scopes, "openid email profile");
assert_eq!(icon_url.as_deref(), Some("/assets/github.svg"));
assert_eq!(return_path, "/settings?tab=login");
```

产品侧仍然负责：

- 把 `ExternalAuthError` 映射成产品错误码和 HTTP response。
- 决定 URL / issuer / return path 的最大长度。
- 构建 callback redirect URI，因为它需要读取产品 runtime config、当前 request host 和产品 API 错误码。
- 本地邮箱格式校验、账号绑定、自动创建用户、审计和 session 写入。

不要在产品仓库保留只重复这些规则的实现；如果产品要保留同名 helper，应该只做错误映射或注入产品常量。

## 自定义 provider

外部系统如果要注册自定义连接器，产品可以创建自己的 `ExternalAuthProviderRegistry`：

```rust
let mut registry = aster_forge_external_auth::ExternalAuthProviderRegistry::new();
registry.register(driver)?;
```

不要修改 Forge 的 `default_registry()` 来塞产品私有 provider。默认 registry 只放通用或明确共享的连接器。

## 错误边界

`ExternalAuthError` 会表达配置、协议、网络、profile 解析等失败。产品侧应该映射为：

- 管理端 provider 测试错误。
- 用户登录失败提示。
- 内部错误日志。
- 审计记录。

不要把 provider 返回的原始错误不加处理地显示给普通用户。

## 测试要求

- registry 能按 kind 找到启用 feature 下的 provider。
- 未启用 feature 的 provider 不应出现在默认 registry。
- OIDC/OAuth2 配置缺字段时返回可诊断错误。
- callback state/code verifier 的产品侧存取要有集成测试。
- 专用 provider 的 option 解析要覆盖默认值和非法值。

## 参考项目

- AsterDrive：外部认证配置页、provider 测试、专用连接器接入。
- AsterYggdrasil：较轻的外部认证登录链路，适合看 Forge driver 到产品用户绑定的边界。
