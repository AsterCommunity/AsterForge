# 认证

AsterForge 默认提供本地认证、会话管理、首个管理员初始化和外部认证 provider 框架。下游项目可以在此基础上扩展角色、权限和组织模型。

## 首个管理员

新实例第一次启动后，需要创建首个管理员：

```bash
curl -X POST http://127.0.0.1:3000/api/v1/auth/setup \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","email":"admin@example.com","password":"change-me-please"}'
```

这个接口只应该在尚未存在管理员时成功。创建完成后，管理面板和 Admin API 都需要管理员身份。

## 本地登录流程

常用端点：

```text
POST /api/v1/auth/register
POST /api/v1/auth/login
POST /api/v1/auth/refresh
POST /api/v1/auth/logout
GET  /api/v1/auth/me
GET  /api/v1/auth/sessions
```

登录后服务会建立认证会话。refresh、logout 和 session 查询都基于会话状态工作。过期或撤销的会话会由 primary runtime 的 cleanup loop 定期清理。

## Cookie 安全

生产环境应该放在 HTTPS 后面，并保持 secure cookie。只在本地 HTTP 调试时使用：

```bash
ASTER__AUTH__BOOTSTRAP_INSECURE_COOKIES=true cargo run
```

这个开关不要带到生产环境。真实部署里应使用 HTTPS、反向代理安全头和正确的 trusted proxy 配置。

## 外部认证

外部认证 provider 用于接入 OIDC/OAuth2 一类身份源。公开认证端点用于登录流程：

```text
GET  /api/v1/external-auth/providers
POST /api/v1/external-auth/{provider}/start
GET  /api/v1/external-auth/{provider}/callback
```

兼容分组路径：

```text
GET  /api/v1/auth/external-auth/providers
GET  /api/v1/auth/external-auth/{kind}/providers
POST /api/v1/auth/external-auth/{kind}/{provider}/start
GET  /api/v1/auth/external-auth/{kind}/{provider}/callback
```

Admin API 用于维护 provider：

```text
GET    /api/v1/admin/external-auth/provider-kinds
GET    /api/v1/admin/external-auth/providers
POST   /api/v1/admin/external-auth/providers
GET    /api/v1/admin/external-auth/providers/{id}
PATCH  /api/v1/admin/external-auth/providers/{id}
DELETE /api/v1/admin/external-auth/providers/{id}
POST   /api/v1/admin/external-auth/providers/test
POST   /api/v1/admin/external-auth/providers/{id}/test
```

外部认证 login flow 会持久化临时状态。已消费或过期的 flow 会由 primary runtime 清理。

## 审计要求

认证相关操作应该写审计，尤其是：

- 用户登录。
- 管理员创建、更新、删除外部认证 provider。
- provider 测试。
- 配置影响认证策略时的变更。

审计 details 不应该泄露 secret。保存 provider secret 时只写摘要或状态，不要把敏感值放进 audit log。

## 下游扩展

如果项目需要更复杂的权限模型，建议把它作为下游业务模块处理：

- AsterForge 保留管理员中间件和基础身份。
- 下游项目增加角色、资源权限、组织归属或租户边界。
- 所有新增 Admin API 都明确接入 admin middleware。
- 普通用户可见的数据和管理员可见的数据分开建模。

后台任务尤其不要默认暴露给普通用户。任务记录可能包含系统维护信息、失败原因和内部 payload，下游项目需要用户任务 API 时，应单独设计可见性规则。
