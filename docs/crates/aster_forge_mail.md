# aster_forge_mail

`aster_forge_mail` 提供产品无关的邮件基础机械层。它不拥有 SMTP 配置、模板、收件人、用户上下文、审计记录或数据库实体。

当前这个 crate 主要覆盖各服务里重复出现的 outbox 投递机制：

- SMTP / sender / 模板配置值的产品无关规范化；
- 投递计数统计；
- 重试延迟策略；
- 投递错误截断；
- SMTP 成功后对 `mark_sent` 的最佳努力重试。
- 邮件 envelope / body 数据模型；
- 邮件模板注册、变量元数据和 placeholder 渲染。

## 适用场景

- 产品有自己的 `mail_outbox` 表，希望复用 dispatch 统计。
- 产品希望在临时投递失败后采用统一的重试延迟。
- 产品希望对持久化错误做 UTF-8 安全截断。
- 产品希望在 SMTP 已接受邮件后，对 `mark_sent` 做共享重试。
- 产品希望复用 `MailRecipient` / `MailMessage` 这类不带发送语义的数据模型。
- 产品希望用注册式方式维护模板变量，并复用 HTML escape、placeholder 替换和 text fallback 生成。
- 产品希望复用 SMTP host/port、sender address/name、mail security、模板 subject/body 的规范化规则。

不适合放在这里的内容：

- 产品模板和模板 code。
- 产品配置 key 和默认值。
- 审计动作。
- 用户 ID 或用户上下文。
- runtime 配置 key。
- 具体 SeaORM repository。
- 具体 `MailSender` trait、SMTP transport、测试 sender。
- 具体 payload enum、业务 URL 生成和本地化文案。

## Config Normalization

模块：`aster_forge_mail::config`

主要类型和函数：

- `MailConfigError`
- `MailConfigResult<T>`
- `parse_smtp_port(value)`
- `normalize_smtp_host_config_value(value)`
- `normalize_smtp_port_config_value(value)`
- `normalize_mail_address_config_value(value)`
- `normalize_mail_name_config_value(value)`
- `normalize_mail_security_config_value(value)`
- `normalize_mail_template_subject_config_value(key, value)`
- `normalize_mail_template_body_config_value(key, value)`
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

产品侧仍然负责：

- 配置 key、默认值、敏感标记和前端 schema；
- runtime settings struct；
- 是否允许未配置 SMTP；
- SMTP transport；
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

## Outbox

模块：`aster_forge_mail::outbox`

主要类型和函数：

- `DispatchStats`
- `MailOutboxRetryPolicy`
- `MailOutboxDeliveryFailureDecision`
- `DEFAULT_ERROR_MAX_LEN`
- `DEFAULT_MARK_SENT_RETRY_DELAYS_MS`
- `retry_delay_secs(attempt_count)`
- `truncate_error(error, max_len)`
- `retry_mark_sent(id, retry_delays_ms, mark_sent)`

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

产品侧仍然负责：

- `MailSender` trait 和测试 sender；
- SMTP / API mail provider transport；
- 地址校验和 transport-specific mailbox 转换；
- 错误映射、审计、metrics 和 outbox 状态。

## Template Registry

模块：`aster_forge_mail::template`

主要类型和函数：

- `MailTemplateDefinition`
- `MailTemplateRegistry`
- `MailTemplateCatalog`
- `MailTemplateCatalogBuilder`
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

- AsterYggdrasil：模板 code、payload enum、审计、SeaORM repository 和 `MailSender` 留在产品代码里，而 dispatch stats、投递失败 decision、重试策略、截断、`mark_sent` 重试、模板 registry、placeholder 渲染和 HTML/text 转换走 Forge。
