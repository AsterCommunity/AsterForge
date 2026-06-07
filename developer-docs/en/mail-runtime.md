# Mail Runtime Extension

The mail system is generic AsterForge runtime infrastructure, not a product workflow. It provides SMTP delivery, template rendering, a durable outbox, primary-only dispatch, administrator test mail, and audit integration. Downstream projects should reuse this path for registration activation, password reset, invitations, alerts, or notification mail.

## Code Layout

Main modules:

| Path | Responsibility |
| --- | --- |
| `src/config/mail.rs` | Runtime mail settings, default templates, normalizers. |
| `src/config/mail_templates/` | Built-in subject and HTML templates. |
| `src/services/mail_template.rs` | Template payloads, variable lists, rendering, link construction. |
| `src/services/mail_service.rs` | SMTP sender, memory sender, test mail, lettre transport. |
| `src/entities/mail_outbox.rs` | `mail_outbox` SeaORM entity. |
| `src/db/repository/mail_outbox_repo.rs` | Outbox claim, mark sent, retry, failed updates and queries. |
| `src/services/mail_outbox_service.rs` | Enqueue, dispatch due, retry backoff, success/failure audit. |
| `src/services/mail_audit_service.rs` | Mail audit details and write helpers. |
| `src/runtime/tasks.rs` | `mail-outbox-dispatch` periodic task registration. |
| `src/runtime/startup/primary.rs` | Primary runtime uses the real SMTP sender. |
| `src/runtime/startup/follower.rs` | Follower initializes a sender but does not start primary-only dispatch loops. |

## Runtime Config

Mail config definitions live in `src/config/definitions.rs` under:

- `mail.config`
- `mail.template`
- `runtime.mail`

When adding a config key:

1. Add the key, schema type, default, category, and sensitivity flag in `definitions.rs`.
2. Add a reader or normalizer in `src/config/mail.rs`.
3. Register the normalizer in `src/config/system_config.rs`.
4. Update the frontend settings surface if the Admin UI needs to display it.
5. If the API contract changes, regenerate OpenAPI and frontend types.

The existing SMTP username/password contract is strict: both must be empty or both must be set. Do not bypass that check in product code.

## Templates And Payloads

Mail template codes are defined in `src/types/mail.rs`:

```text
register_activation
contact_change_confirmation
password_reset
password_reset_notice
contact_change_notice
external_auth_email_verification
login_email_code
```

When adding a template:

1. Add the enum variant, SeaORM string value, and `as_str()` mapping in `MailTemplateCode`.
2. Add subject and HTML template files under `src/config/mail_templates/`.
3. Register subject/html keys and defaults in `src/config/mail.rs`.
4. Add subject/html runtime config keys in `src/config/definitions.rs`.
5. Add payload type, variable list, render branch, and unit tests in `src/services/mail_template.rs`.
6. Confirm `MailTemplateCode` and related DTO/schema types are registered in `src/api/openapi.rs`.
7. Regenerate frontend API types and add frontend labels if needed.

Template rendering must keep text and HTML values separate. User-controlled values must be escaped before entering HTML body output.

## Outbox And Delivery

Product flows should not send SMTP directly on the request path. Use:

```text
mail_outbox_service::enqueue(...)
```

This moves external side effects into the background runtime and avoids coupling request success to transient SMTP availability.

Delivery semantics:

- `list_claimable()` selects `pending`, due `retry`, and stale `processing` rows.
- `try_claim()` moves a row to `processing`.
- After SMTP succeeds, `mark_sent_with_retry()` runs. `mail_send` audit is written only after the sent state is persisted.
- Non-final failures move the row to `retry` using `retry_delay_secs()` backoff.
- Exhausted failures move the row to `failed` and write `mail_delivery_failed` audit.

The risky case is the duplicate-send window: SMTP succeeds but database `mark_sent` fails. The current implementation retries `mark_sent` briefly to reduce the chance of the next dispatch sending the same message again. Any change here needs regression coverage.

## Runtime Integration

`mail-outbox-dispatch` is `SystemRuntimeTaskKind::MailOutboxDispatch`; its presentation code is `runtime_task_mail_outbox_dispatch`.

Registration points:

- `src/services/task_service/runtime.rs`
- `src/services/task_service/types.rs`
- `src/runtime/tasks.rs`
- `frontend-panel/src/lib/presentation.ts`

It is a primary-only task. Do not start outbox dispatch from follower nodes. Mail delivery is an external side effect, and duplicate delivery across nodes will damage user experience and audit clarity.

## Admin Action And OpenAPI

Administrator test mail uses a config action:

```text
POST /api/v1/admin/config/mail/action
```

Related code:

- `src/services/config_service/actions.rs`
- `src/api/dto/admin.rs`
- `src/api/routes/admin/config.rs`
- `src/api/openapi.rs`

When adding a config action:

1. Extend `ConfigActionType`.
2. Implement the action in the service layer and decide whether it needs audit.
3. Add route OpenAPI annotations.
4. Register schemas in `src/api/openapi.rs`.
5. Regenerate OpenAPI and frontend types.
6. Update the Admin UI caller.

## Audit Contract

Mail audit actions:

- `mail_send`
- `mail_delivery_failed`

Registration points:

- `src/types/audit.rs`
- `src/services/audit_service/details.rs`
- `src/services/audit_service/presentation.rs`
- `src/services/mail_audit_service.rs`
- `frontend-panel/src/lib/presentation.ts`

Current mail detail fields:

```text
to_address
template_code
to_name
subject
outbox_id
attempt_count
error
```

When adding fields, update backend presentation and frontend presentation fallback together. Frontend code should not parse raw detail strings.

## Tests And Generation

Useful commands:

```bash
cargo test mail_template
cargo test --test test_audit mail_outbox_dispatch_records_delivery_audit_logs
cargo test --features openapi generate_openapi

cd frontend-panel
bun run generate-api
bun run check
```

When changing outbox dispatch semantics, cover at least:

- Successful delivery records `mail_send`.
- Final failure records `mail_delivery_failed`.
- Retryable failure does not record final failure audit early.
- `mark_sent` failure does not record success audit.
- OpenAPI generation and frontend types still pass.
