# Audit And Background Tasks

Audit logs and background tasks are core reusable runtime capabilities in AsterForge. The goal is not just to have tables. The goal is to let administrators understand what happened, why a task failed, and which operations need accountability.

## Audit Logs

Audit logs record important operations:

- Server startup and shutdown.
- User login.
- Runtime config changes.
- Admin config deletion.
- Admin external auth provider maintenance.
- Administrator test mail.
- Successful mail send and final delivery failure.
- Admin task cleanup.
- Background task retry.

Admin API:

```text
GET /api/v1/admin/audit-logs
```

Queries support pagination and filtering. The admin panel displays structured presentation data. Presentation matters because the frontend should not parse raw details or guess field meanings for every action.

## Presentation

Audit presentation uses stable message codes and parameters.

Recommended shape:

```json
{
  "code": "audit.config.updated",
  "params": {
    "key": "auth.registration_enabled"
  }
}
```

The frontend can localize display from code and params. The backend can still keep raw details for debugging and historical compatibility.

When adding an audit action, add:

- Stable action name.
- Details structure.
- Presentation mapping.
- Admin query display coverage.
- Unit or integration tests.

Current mail-related presentation codes:

| Action | Presentation code | Important params |
| --- | --- | --- |
| `mail_send` | `mail_sent` | `to_address`, `template_code`, `outbox_id` |
| `mail_delivery_failed` | `mail_delivery_failed` | `to_address`, `template_code`, `outbox_id`, `attempt_count`, `error` |
| `config_action_execute` | `config_action_executed` | `action`, `target_email` |

When an administrator sends test mail, AsterForge records `config_action_execute`. A successful test also records `mail_send`; a failed test records `mail_delivery_failed`. Outbox background delivery also records mail audit on successful send or final failure.

## Background Tasks

The background task system persists system task state and supports dispatch, lease, heartbeat, retry, and cleanup.

Admin API:

```text
GET  /api/v1/admin/tasks
POST /api/v1/admin/tasks/cleanup
POST /api/v1/admin/tasks/{id}/retry
```

The template does not provide regular user task APIs. Admins can see system tasks, but that does not mean regular users should. If a downstream project needs a "my tasks" page, design product-specific visibility rules.

## Task State

Task records usually include:

- display name or task kind
- status
- creator user id
- payload
- result
- failure detail
- retry metadata
- timestamps
- presentation

Presentation provides stable title, status message, and detail message. The frontend should not parse payload to infer titles, and failure detail should not be the only display source.

## Cleanup Strategy

AsterForge includes these maintenance tasks:

- audit log cleanup
- task artifact cleanup
- auth session cleanup
- external auth login flow cleanup
- mail outbox dispatch

Administrators can also trigger task cleanup through the Admin Task Cleanup API. That operation itself should be audited because it deletes historical task data.

`mail-outbox-dispatch` is an external-side-effect task and runs only on primary nodes. It claims due rows from `mail_outbox`, sends SMTP, and marks each row as `sent`, `retry`, or `failed`. If this task keeps failing, administrators should inspect both task failure details and `mail_delivery_failed` audit entries.

Cleanup policy should consider:

- Whether succeeded and failed tasks use different retention periods.
- Whether recent failures are kept for debugging.
- Whether task artifacts are removed only after the task has stopped.
- Whether cleanup actions write audit logs.

## New Task Checklist

When adding a downstream background task:

1. Define task kind and payload structure.
2. Decide payload version compatibility.
3. Implement the task handler.
4. Define retry classification.
5. Add task presentation.
6. Add Admin UI fields if needed.
7. Cover success, failure, retry, and cleanup in tests.
8. Audit administrator-triggered task operations.

The common failure mode is retrying a task with non-idempotent side effects. If the task calls external systems or mutates business data, design idempotency keys or compensation behavior.
