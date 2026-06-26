# aster_forge_mail

`aster_forge_mail` 提供产品无关的邮件基础机械层。它拥有 Aster 产品共享的邮件状态、模板 code 和投递控制流，但不拥有 SMTP 配置 key、产品模板 payload、用户上下文、审计记录或数据库实体。

当前这个 crate 主要覆盖各服务里重复出现的 outbox 投递机制：

- SMTP / sender / 模板配置值的产品无关规范化；
- SMTP runtime settings 模型、默认端口和 readiness 判定；
- 投递计数统计；
- 共享 Aster `MailTemplateCode`、`MailOutboxStatus` 和 `StoredMailPayload`；
- 重试延迟策略；
- 投递错误截断；
- SMTP 成功后对 `mark_sent` 的最佳努力重试。
- 邮件 envelope / body 数据模型；
- `MailSender` trait、SMTP sender、memory sender；
- 邮件模板注册、变量元数据和 placeholder 渲染。

## 适用场景

- 产品有自己的 `mail_outbox` 表，希望复用 dispatch 统计。
- 产品希望在临时投递失败后采用统一的重试延迟。
- 产品希望对持久化错误做 UTF-8 安全截断。
- 产品希望在 SMTP 已接受邮件后，对 `mark_sent` 做共享重试。
- 产品希望复用 `MailRecipient` / `MailMessage` 这类不带发送语义的数据模型。
- 产品希望复用统一的 SMTP 发送实现和内存测试 sender。
- 产品希望用注册式方式维护模板变量，并复用 HTML escape、placeholder 替换和 text fallback 生成。
- 产品希望复用 SMTP host/port、sender address/name、mail security、模板 subject/body 的规范化规则。
- 产品希望复用统一的 SMTP runtime settings 结构和“是否可投递”判定。

不适合放在这里的内容：

- 产品模板 payload、渲染逻辑和本地化文案。
- 产品配置 key 和默认值。
- 审计动作。
- 用户 ID 或用户上下文。
- runtime 配置 key 和 runtime config 读取函数。
- 具体 SeaORM repository。
- 具体 payload enum、业务 URL 生成和本地化文案。

## Config Normalization

模块：`aster_forge_mail::config`

主要类型和函数：

- `MailConfigError`
- `MailConfigResult<T>`
- `MailRuntimeSettings`
- `parse_smtp_port(value)`
- `normalize_smtp_host_config_value(value)`
- `normalize_smtp_port_config_value(value)`
- `normalize_mail_address_config_value(value)`
- `normalize_mail_name_config_value(value)`
- `normalize_mail_security_config_value(value)`
- `normalize_mail_template_subject_config_value(key, value)`
- `normalize_mail_template_body_config_value(key, value)`
- `DEFAULT_MAIL_SMTP_PORT`
- `DEFAULT_MAIL_SECURITY`
- `MAIL_TEMPLATE_MAX_SUBJECT_LEN`
- `MAIL_TEMPLATE_MAX_BODY_LEN`

这些 helper 只处理产品无关的存储值规范化：

- SMTP host 会 trim、转小写，空值表示未配置，非空值不能包含空白字符。
- SMTP port 必须是 `1..=65535` 的整数。
- sender address 会 trim、转小写，空值表示未配置，非空值必须符合 Aster 轻量 email 规则。
- sender name 会 trim，最长 128 字符。
- mail security 接受 `true/false`、`1/0`、`yes/no`、`on/off`，存储为 `true` / `false`。
- template subject 会 trim，不能为空，不能包含换行，最长 255 字符。
- template body 会把 CRLF/CR 规范化成 LF，不能为空，最长 64 KiB。

`MailRuntimeSettings` 是发送前的产品无关 SMTP 设置快照：

- `is_configured()` 要求 `smtp_host` 和 `from_address` 非空；
- `is_ready_for_delivery()` 在已配置基础上，要求 `smtp_username` 和 `smtp_password` 同时为空或同时非空。

产品侧仍然负责：

- 配置 key、默认值、敏感标记和前端 schema；
- 从自己的 runtime config 读取并构造 `MailRuntimeSettings`；
- 是否允许在业务流程中静默跳过未配置 SMTP；
- `MailConfigError` 到产品 API 错误码的映射。

示例：

```rust
pub fn normalize_mail_security_config_value(value: &str) -> Result<String> {
    aster_forge_mail::normalize_mail_security_config_value(value)
        .map_err(|error| AsterError::validation_error(error.to_string()))
}
```

## Cargo

```toml
[dependencies]
aster_forge_mail = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Sender

模块：`aster_forge_mail::sender`

主要类型和函数：

- `MailSender`
- `MailDeliveryError`
- `MailSendResult<T>`
- `SmtpMailSender`
- `MemoryMailSender`
- `smtp_sender(settings_provider)`
- `memory_sender()`
- `memory_sender_ref(sender)`
- `send_rendered_with(sender, settings, to, rendered)`
- `DEFAULT_SMTP_SEND_TIMEOUT_SECS`

`SmtpMailSender` 在每次投递前调用 `settings_provider`，因此产品侧可以继续从自己的 runtime config 快照读取最新 SMTP 设置：

```rust
pub fn runtime_sender(runtime_config: Arc<RuntimeConfig>) -> Arc<dyn aster_forge_mail::MailSender> {
    aster_forge_mail::smtp_sender(move || runtime_mail_settings(&runtime_config))
}
```

`send_rendered_with` 只负责把 `RenderedMail` 和 `MailRuntimeSettings` 中的 sender identity 组装成 `MailMessage`，再交给传入的 sender。产品侧仍然负责把 `MailDeliveryError` 映射成自己的错误类型：

```rust
fn map_mail_delivery_error(error: aster_forge_mail::MailDeliveryError) -> AsterError {
    match error {
        aster_forge_mail::MailDeliveryError::NotConfigured(message) => {
            AsterError::mail_not_configured(message)
        }
        aster_forge_mail::MailDeliveryError::InvalidMessage(message) => {
            AsterError::validation_error(message)
        }
        aster_forge_mail::MailDeliveryError::Config(message) => AsterError::config_error(message),
        aster_forge_mail::MailDeliveryError::Delivery(message) => {
            AsterError::mail_delivery_failed(message)
        }
        aster_forge_mail::MailDeliveryError::Internal(message) => AsterError::internal_error(message),
    }
}
```

## Outbox

模块：`aster_forge_mail::outbox`

主要类型和函数：

- `DispatchStats`
- `MailTemplateCode`
- `MailOutboxStatus`
- `StoredMailPayload`
- `MailOutboxDispatchConfig`
- `MailOutboxDispatchRow`
- `MailOutboxRetryPolicy`
- `MailOutboxDeliveryFailureDecision`
- `dispatch_mail_outbox(config, callbacks...)`
- `drain_mail_outbox(config, dispatch)`
- `DEFAULT_ERROR_MAX_LEN`
- `DEFAULT_MARK_SENT_RETRY_DELAYS_MS`
- `retry_delay_secs(attempt_count)`
- `truncate_error(error, max_len)`
- `retry_mark_sent(id, retry_delays_ms, mark_sent)`

`MailTemplateCode` 只标准化当前 Aster 产品共享的持久化模板 code，例如
`password_reset`、`external_auth_email_verification` 和 `user_invitation`。产品仍然维护自己的
payload enum、模板文件、链接构造、变量列表和渲染分支。共享数据库 schema 给
`template_code` 预留 64 字节；新增共享模板 code 时要保持稳定、snake_case，并避免超过这个长度。

### 统计和策略

`DispatchStats` 是一个轻量计数器：

```rust
use aster_forge_mail::DispatchStats;

let mut total = DispatchStats::default();
total.merge(DispatchStats {
    claimed: 1,
    sent: 1,
    retried: 0,
    failed: 0,
});
```

`MailOutboxRetryPolicy` 描述产品无关的重试决策。产品代码传入“本次投递后的 attempt count”和原始错误，Forge 返回一个明确的 `MailOutboxDeliveryFailureDecision`：

```rust
use aster_forge_mail::{
    DEFAULT_ERROR_MAX_LEN, MailOutboxDeliveryFailureDecision, MailOutboxRetryPolicy,
};

let policy = MailOutboxRetryPolicy::new(6, DEFAULT_ERROR_MAX_LEN);
let decision = policy.delivery_failure_decision(attempt_count, delivery_error.to_string());

match decision {
    MailOutboxDeliveryFailureDecision::PermanentFailure {
        attempt_count,
        error_message,
    } => {
        // 产品代码标记 failed，并写入自己的审计字段。
    }
    MailOutboxDeliveryFailureDecision::Retry {
        attempt_count,
        retry_delay_secs,
        error_message,
    } => {
        // 产品代码用自己的时钟和数据库层写 next_attempt_at。
    }
}
```

Forge 只决定“永久失败还是重试”和“错误字符串如何截断”。数据库状态、审计、时间戳和事务仍然由产品侧处理。

### Dispatch Loop

`dispatch_mail_outbox` 把 outbox 的控制流收进 Forge：

- 拉取可 claim 的 rows；
- 对每行执行 claim；
- 成功 claim 后调用产品传入的 deliver；
- deliver 成功后用 `retry_mark_sent` 尽力标记 sent；
- deliver 失败后按 `MailOutboxRetryPolicy` 决定 retry / failed；
- 调用产品传入的 audit callback；
- 返回统一的 `DispatchStats`。

产品侧仍然保留具体 `mail_outbox` entity/repository、claimable 查询条件、timestamp 计算、模板渲染、audit 写入和产品错误类型。

产品的 row model 只需要实现最小 metadata trait：

```rust
impl aster_forge_mail::MailOutboxDispatchRow for mail_outbox::Model {
    fn id(&self) -> i64 { self.id }
    fn attempt_count(&self) -> i32 { self.attempt_count }
    fn template_code(&self) -> &str { self.template_code.as_str() }
    fn to_address(&self) -> &str { &self.to_address }
}
```

`drain_mail_outbox` 负责多轮调用产品提供的 dispatch 函数，直到没有 row 被 claim，或者达到 `drain_max_rounds`。

### mark_sent 重试

`retry_mark_sent` 缩小了一个典型窗口：SMTP 已成功，但数据库行仍然是 `Processing`。

```rust
use aster_forge_mail::{DEFAULT_MARK_SENT_RETRY_DELAYS_MS, retry_mark_sent};

let updated = retry_mark_sent(
    outbox_id,
    DEFAULT_MARK_SENT_RETRY_DELAYS_MS,
    |id, _attempt| async move {
        product_mail_outbox_repo::mark_sent(db, id, now()).await
    },
)
.await?;
```

产品回调负责时间戳、repository 调用、错误类型、事务和日志上下文。Forge 只负责重试循环和延迟调度。

## Message Model

模块：`aster_forge_mail::message`

主要类型：

- `MailRecipient`
- `MailMessage`

这两个类型只表达已经渲染好的邮件 envelope 和 body：

```rust
use aster_forge_mail::{MailMessage, MailRecipient};

let message = MailMessage {
    from: MailRecipient {
        address: "no-reply@example.com".to_string(),
        display_name: Some("Aster".to_string()),
    },
    to: MailRecipient {
        address: "user@example.com".to_string(),
        display_name: None,
    },
    subject: "Welcome".to_string(),
    text_body: "Welcome to Aster.".to_string(),
    html_body: "<p>Welcome to Aster.</p>".to_string(),
};
```

产品侧仍然负责错误映射、审计、metrics、outbox 状态和业务投递策略。

## Template Registry

模块：`aster_forge_mail::template`

主要类型和函数：

- `MailTemplateDefinition`
- `MailTemplateRegistry`
- `MailTemplateCatalog`
- `MailTemplateCatalogBuilder`
- `MailTemplateRegistrar`
- `MailTemplateRegistryError`
- `TemplateVariableSpec`
- `TemplateVariableGroup`
- `TemplateVariableItem`
- `TemplatePlaceholderSet`
- `RenderedMail`
- `render_template(subject_template, html_template, placeholders)`
- `render_placeholders(template, values)`
- `escape_html(value)`
- `html_to_text(html)`

Forge 不定义任何产品模板 code，也不解析产品 payload。产品侧把自己的模板注册成静态定义：

```rust
use aster_forge_mail::{MailTemplateDefinition, MailTemplateRegistry, TemplateVariableSpec};

const USERNAME: TemplateVariableSpec = TemplateVariableSpec::new(
    "username",
    "settings_template_variable_username_label",
    "settings_template_variable_username_desc",
);
const SITE_NAME: TemplateVariableSpec = TemplateVariableSpec::new(
    "site_name",
    "settings_template_variable_site_name_label",
    "settings_template_variable_site_name_desc",
);
const WELCOME_VARIABLES: &[TemplateVariableSpec] = &[USERNAME, SITE_NAME];
const DEFINITIONS: &[MailTemplateDefinition] = &[MailTemplateDefinition::new(
    "welcome",
    "mail_template",
    "settings_mail_template_group_welcome",
    WELCOME_VARIABLES,
)];

let registry = MailTemplateRegistry::new(DEFINITIONS);
registry.validate()?;
let groups = registry.variable_groups();
```

如果一个产品由多个子系统分别提供模板，可以用 `MailTemplateCatalogBuilder` 做组合注册。这样主程序只负责装配，子系统自己导出自己的静态 definitions：

```rust
use aster_forge_mail::{MailTemplateCatalog, MailTemplateDefinition};

static ACCOUNT_TEMPLATE: MailTemplateDefinition = MailTemplateDefinition::new(
    "account_welcome",
    "mail_template",
    "settings_mail_template_group_account_welcome",
    &[],
);

static BILLING_TEMPLATE: MailTemplateDefinition = MailTemplateDefinition::new(
    "billing_invoice",
    "mail_template",
    "settings_mail_template_group_billing_invoice",
    &[],
);

let mut builder = MailTemplateCatalog::builder();
builder.register(&ACCOUNT_TEMPLATE);
builder.register(&BILLING_TEMPLATE);
let catalog = builder.build()?;
```

如果子系统导出注册函数，可以让 Forge 统一执行这些 registrar：

```rust
fn register_account_templates(builder: &mut MailTemplateCatalogBuilder) {
    builder.register(&ACCOUNT_TEMPLATE);
}

fn register_billing_templates(builder: &mut MailTemplateCatalogBuilder) {
    builder.register(&BILLING_TEMPLATE);
}

let catalog = MailTemplateCatalog::from_registrars(&[
    register_account_templates,
    register_billing_templates,
])?;
```

`build()` 会校验：

- 模板 code 不能为空；
- 模板 code 不能重复；
- 同一个模板里的变量 key 不能为空；
- 同一个模板里的变量 key 不能重复。

静态 `MailTemplateRegistry` 适合一个模块集中维护模板列表；`MailTemplateCatalogBuilder` 适合以后 Forge/Yggdrasil/Drive 这种多子系统注册模板的接入方式。

产品侧仍然负责：

- 从 runtime config 读取 subject/html 模板。
- 把数据库里的 payload JSON 解码成自己的 payload 类型。
- 生成 verification/reset/invitation 等业务 URL。
- 决定哪些变量进入 text 渲染，哪些变量需要 HTML escape。

渲染时只把机械层交给 Forge：

```rust
use aster_forge_mail::{TemplatePlaceholderSet, escape_html, render_template};

let placeholders = TemplatePlaceholderSet::new(
    vec![("username", "Alice".to_string())],
    vec![("username", escape_html("Alice"))],
);
let rendered = render_template(subject_template, html_template, &placeholders);
```

`render_template` 会使用 text values 渲染 subject，用 HTML values 渲染 HTML body，并从 HTML
生成 plain-text fallback。

## 错误边界

`retry_mark_sent` 返回产品回调的错误类型。这样数据库和 API 错误映射继续留在产品 crate。

## 测试

Forge 测试覆盖：

- dispatch 计数合并；
- 默认重试延迟策略；
- 投递失败 decision 分类；
- UTF-8 安全截断；
- `retry_mark_sent` 的重试成功和最终失败行为。
- 模板 registry 的变量组生成；
- placeholder 渲染、HTML escape 和 text fallback。

产品测试仍然应该覆盖：

- repository claim fence；
- stale processing reclaim；
- 模板渲染；
- 模板 payload 编解码和业务 URL 生成；
- 审计记录；
- SMTP sender 配置；
- 事务行为。

## 参考项目

- AsterYggdrasil：模板 code、payload enum、审计和 SeaORM repository 留在产品代码里；sender、dispatch stats、投递失败 decision、重试策略、截断、`mark_sent` 重试、模板 registry、placeholder 渲染和 HTML/text 转换走 Forge。
