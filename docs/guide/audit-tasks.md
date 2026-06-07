# 审计与后台任务

审计日志和后台任务是 AsterForge 里最重要的通用运行时能力之一。它们的目标不是“有个表能查”，而是让管理员能稳定理解系统发生了什么、后台任务为什么失败、哪些操作需要追责。

## 审计日志

审计日志记录关键操作：

- 服务启动和关闭。
- 用户登录。
- 运行时配置变更。
- 管理员删除配置。
- 管理员维护外部认证 provider。
- 管理员发送测试邮件。
- 邮件发送成功和最终投递失败。
- 管理员清理任务。
- 后台任务重试。

Admin API：

```text
GET /api/v1/admin/audit-logs
```

查询支持分页和过滤，前端管理面板会展示结构化 presentation。presentation 的存在很关键：前端不需要解析 raw details，也不需要猜不同 action 的字段含义。

## Presentation

审计 presentation 使用稳定 message code 和参数表达展示信息。

推荐形态：

```json
{
  "code": "audit.config.updated",
  "params": {
    "key": "auth.registration_enabled"
  }
}
```

前端可以根据 code 和 params 显示本地化文案。后端仍保留 raw details，方便排查和兼容历史数据。

新增审计 action 时，不要只写一段字符串。至少应该补：

- action 常量或稳定 action name。
- details 结构。
- presentation 映射。
- 管理员查询时的展示覆盖。
- 测试或集成测试覆盖。

当前邮件相关 presentation code：

| Action | Presentation code | 关键参数 |
| --- | --- | --- |
| `mail_send` | `mail_sent` | `to_address`、`template_code`、`outbox_id` |
| `mail_delivery_failed` | `mail_delivery_failed` | `to_address`、`template_code`、`outbox_id`、`attempt_count`、`error` |
| `config_action_execute` | `config_action_executed` | `action`、`target_email` |

管理员执行测试邮件时，会记录 `config_action_execute`。测试邮件发送成功会额外记录 `mail_send`；失败会记录 `mail_delivery_failed`。Outbox 周期投递成功或最终失败也会写邮件审计。

## 后台任务

后台任务系统用于持久化系统任务状态，并支持 dispatcher、lease、heartbeat、retry 和 cleanup。

Admin API：

```text
GET  /api/v1/admin/tasks
POST /api/v1/admin/tasks/cleanup
POST /api/v1/admin/tasks/{id}/retry
```

当前模板不提供普通用户任务 API。管理员可以看到系统任务，不代表普通用户也应该看到。下游项目如果需要“我的任务”页面，必须按业务可见性单独设计。

## 任务状态

任务记录通常包含：

- display name 或 task kind。
- status。
- creator user id。
- payload。
- result。
- failure detail。
- retry 信息。
- timestamps。
- presentation。

presentation 会提供稳定标题、状态消息和详情消息。前端不应该解析 payload 来判断任务标题，也不应该把 failure detail 当成唯一展示来源。

## 清理策略

AsterForge 已经包含这些维护任务：

- audit log cleanup
- task artifact cleanup
- auth session cleanup
- external auth login flow cleanup
- mail outbox dispatch

管理员也可以通过 Admin Task Cleanup API 触发任务清理。这个操作本身需要审计，因为它会删除历史任务数据。

其中 `mail-outbox-dispatch` 是外部副作用任务，只在 primary 节点运行。它从 `mail_outbox` claim 到期邮件、投递 SMTP、按结果标记 `sent`、`retry` 或 `failed`。如果这个任务持续失败，管理员应该同时看任务失败详情和邮件审计里的 `mail_delivery_failed`。

清理策略应考虑：

- 成功任务和失败任务是否使用不同保留时间。
- 是否保留最近失败记录用于排查。
- 删除任务 artifact 前是否确认任务已经终止。
- 清理动作本身是否写入 audit log。

## 新增任务 checklist

下游项目新增后台任务时，建议按这个顺序做：

1. 定义 task kind 和 payload 结构。
2. 明确 payload 版本兼容策略。
3. 实现任务 handler。
4. 定义 retry classification。
5. 补 task presentation。
6. 补 Admin UI 展示需要的字段。
7. 补任务成功、失败、重试和清理测试。
8. 对管理员触发的任务操作写 audit log。

任务系统最容易出问题的地方是“失败后重试造成重复副作用”。如果任务会调用外部系统或修改业务数据，务必设计幂等键或补偿逻辑。
