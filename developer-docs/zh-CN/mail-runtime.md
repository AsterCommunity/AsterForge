# 邮件运行时扩展

邮件系统是 AsterForge 的通用运行时能力，不是某个产品流程。它提供 SMTP 发送、模板渲染、持久化 outbox、primary-only 投递、Admin 测试邮件和 audit 集成。下游项目需要注册激活、密码重置、邀请、告警或通知邮件时，应复用这套路径。

## 代码结构

主要模块：

| 路径 | 职责 |
| --- | --- |
| `src/config/mail.rs` | 邮件运行时配置读取、默认模板、normalizer。 |
| `src/config/mail_templates/` | 内置 subject 和 HTML 模板。 |
| `src/services/mail_template.rs` | 模板 payload、变量列表、渲染和链接构造。 |
| `src/services/mail_service.rs` | SMTP sender、内存 sender、测试邮件、lettre transport。 |
| `src/entities/mail_outbox.rs` | `mail_outbox` SeaORM 实体。 |
| `src/db/repository/mail_outbox_repo.rs` | outbox claim、mark sent、retry、failed 查询和更新。 |
| `src/services/mail_outbox_service.rs` | enqueue、dispatch due、重试退避、成功/失败 audit。 |
| `src/services/mail_audit_service.rs` | 邮件审计 details 和写入入口。 |
| `src/runtime/tasks.rs` | `mail-outbox-dispatch` 周期任务注册。 |
| `src/runtime/startup/primary.rs` | primary runtime 使用真实 SMTP sender。 |
| `src/runtime/startup/follower.rs` | follower 初始化 sender，但不启动 primary-only 投递 loop。 |

## 运行时配置

邮件配置项定义在 `src/config/definitions.rs`，类别为：

- `mail.config`
- `mail.template`
- `runtime.mail`

新增配置项时，需要同步：

1. 在 `definitions.rs` 增加 key、schema 类型、默认值、分类和敏感标记。
2. 在 `src/config/mail.rs` 增加读取函数或 normalizer。
3. 在 `src/config/system_config.rs` 注册 normalizer。
4. 如果配置会影响 Admin UI，更新前端设置页对应展示。
5. 如果配置 API contract 改变，重新生成 OpenAPI 和前端类型。

SMTP username/password 的现有契约是二者必须同时为空或同时非空。不要在业务层绕过这个判断。

## 模板和 payload

邮件模板 code 定义在 `src/types/mail.rs`：

```text
register_activation
contact_change_confirmation
password_reset
password_reset_notice
contact_change_notice
external_auth_email_verification
login_email_code
```

新增模板时，需要同步：

1. 在 `MailTemplateCode` 增加枚举值、SeaORM string value 和 `as_str()`。
2. 在 `src/config/mail_templates/` 增加 subject 和 HTML 模板文件。
3. 在 `src/config/mail.rs` 的 `template_subject_key()`、`template_html_key()`、默认模板函数里注册。
4. 在 `src/config/definitions.rs` 增加 subject/html runtime config key。
5. 在 `src/services/mail_template.rs` 增加 payload 类型、变量列表、render 分支和单元测试。
6. 在 `src/api/openapi.rs` 确认 `MailTemplateCode` 和相关 DTO/schema 已注册。
7. 重新生成前端 API 类型，必要时补前端文案。

模板渲染必须区分 text 值和 HTML 值。用户输入进入 HTML body 前要走 escaping，不要直接拼。

## Outbox 和投递

业务流程不要直接在请求路径里发 SMTP。应该调用：

```text
mail_outbox_service::enqueue(...)
```

这样可以把外部副作用移到 background runtime，避免请求阻塞和瞬时 SMTP 故障导致业务状态不可恢复。

投递语义：

- `list_claimable()` 只取 `pending`、到期 `retry`、过期 `processing`。
- `try_claim()` 把记录置为 `processing`。
- SMTP 成功后调用 `mark_sent_with_retry()`，只有标记成功才写 `mail_send` audit。
- 失败未耗尽次数时写 `retry`，按 `retry_delay_secs()` 退避。
- 耗尽次数后写 `failed`，再写 `mail_delivery_failed` audit。

这里最容易出问题的是“双发窗口”：SMTP 已经成功，但数据库 `mark_sent` 失败。现有实现会对 `mark_sent` 做短重试，减少下一轮重新发送的概率。修改这段逻辑时必须补回归测试。

## Runtime 集成

`mail-outbox-dispatch` 是 `SystemRuntimeTaskKind::MailOutboxDispatch`，presentation code 是 `runtime_task_mail_outbox_dispatch`。

注册点：

- `src/services/task_service/runtime.rs`
- `src/services/task_service/types.rs`
- `src/runtime/tasks.rs`
- `frontend-panel/src/lib/presentation.ts`

它是 primary-only 任务。不要让 follower 启动 outbox dispatch loop。邮件投递是外部副作用，多节点重复投递会把用户体验和审计都搞乱。

## Admin action 和 OpenAPI

管理员测试邮件走 config action：

```text
POST /api/v1/admin/config/mail/action
```

相关代码：

- `src/services/config_service/actions.rs`
- `src/api/dto/admin.rs`
- `src/api/routes/admin/config.rs`
- `src/api/openapi.rs`

新增 config action 时，必须：

1. 扩展 `ConfigActionType`。
2. 在 service 层实现 action，并决定是否需要 audit。
3. 给 route 写 OpenAPI 注解。
4. 在 `src/api/openapi.rs` 注册 schema。
5. 重新生成 OpenAPI 和前端类型。
6. 更新 Admin UI 调用。

## Audit 契约

邮件 audit action：

- `mail_send`
- `mail_delivery_failed`

相关注册点：

- `src/types/audit.rs`
- `src/services/audit_service/details.rs`
- `src/services/audit_service/presentation.rs`
- `src/services/mail_audit_service.rs`
- `frontend-panel/src/lib/presentation.ts`

邮件 details 当前字段：

```text
to_address
template_code
to_name
subject
outbox_id
attempt_count
error
```

新增字段时，后端 presentation 和前端 presentation fallback 都要一起更新。前端不要解析 raw details 字符串。

## 测试和生成

常用命令：

```bash
cargo test mail_template
cargo test --test test_audit mail_outbox_dispatch_records_delivery_audit_logs
cargo test --features openapi generate_openapi

cd frontend-panel
bun run generate-api
bun run check
```

改 outbox dispatch 语义时，至少覆盖：

- 成功投递后记录 `mail_send`。
- 最终失败后记录 `mail_delivery_failed`。
- retry 失败不会提前写最终失败审计。
- `mark_sent` 失败时不会写成功审计。
- OpenAPI 生成和前端类型仍然通过。

代码不测就是不负责，尤其是邮件这种会打到外部系统的东西。
