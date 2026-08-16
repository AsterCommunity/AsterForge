# aster_forge_test

`aster_forge_test` 提供 Aster 产品共享的集成测试基础设施：隔离临时目录、文件型 SQLite fixture、可复用 Docker 容器（testcontainers）、跨进程容器状态登记、真实子进程 guard 和异步等待工具。目标是让每个产品的测试套件不必重复实现测试资源命名、生命周期清理、容器复用、状态锁、孤儿资源清理和进程日志收敛。

这是测试支持 crate，`publish = false`，只作为 dev-dependency 使用。所有失败路径直接 panic，因为测试设施出错时测试本来就不该继续。

## 适用场景

- 集成测试需要真实 Redis / PostgreSQL / MySQL / Mailpit，而不是 mock。
- 多次 `cargo test` 之间复用同一个容器，避免每次启动几秒的开销。
- 多个 checkout / worktree 并行跑测试，容器名不能互相打架。
- 测试进程在容器里创建了 per-test 资源（如独立数据库），后续运行需要清理已退出进程留下的孤儿资源。
- E2E 需要启动一个或多个真实服务进程，并在失败时自动附带 stdout/stderr 尾部。
- 测试需要跨平台唯一临时目录，或需要把 SQLite 主文件与 WAL/SHM/journal sidecar 一起清理。

不适合放在这里的内容：

- 产品自己的测试模型、fixture、seed 数据。
- 产品语义的资源命名规范（数据库名、key 前缀仍由产品侧决定）。
- CI 编排逻辑（service container 怎么起是 CI 的事）。

## 临时文件系统 fixture

`temp` 模块默认可用，不需要 Cargo feature：

```rust
use aster_forge_test::temp::{SqliteTestDatabase, TestTempDir};

let directory = TestTempDir::new("config-loader");
let config_path = directory.join("data/config.toml");

let database = SqliteTestDatabase::new("repository-case");
let db = sea_orm::Database::connect(database.url()).await?;
// ... assertions ...
db.close().await?;
```

- `TestTempDir::new(scope)` 在平台临时目录下创建带 PID、进程内计数器和时间戳的隔离目录。
- `TestTempDir::new_in(root, scope)` 用于必须位于项目目录下的测试，例如验证相对配置路径；清理仍由 guard 负责。
- `TestTempDir` 复用 `aster_forge_utils::raii::TempDirGuard`，不重复实现资源回收。
- `SqliteTestDatabase` 把数据库放进独立目录，drop 时主文件、journal、WAL 和 SHM sidecar 会随目录一起清理。
- SQLite URL 使用 percent-encoded `sqlite:` opaque path，Windows drive letter、反斜杠、空格和 URL 保留字符不会改变文件名语义。
- Windows 会锁住仍然打开的 SQLite 文件；测试必须先显式关闭连接池，再让 fixture 离开作用域。

## Cargo feature

无默认 feature。

- `containers`：启用共享容器状态机（fs2 文件锁、PID 登记、租约清理）。
- `process`：启用真实子进程 guard 和 loopback 临时端口分配。
- `redis` / `postgres` / `mysql`：启用对应数据服务的容器 helper，各自隐含 `containers`。
- `smtp`：启用 Mailpit 容器、SMTP endpoint、消息清理和消息计数 API，隐含 `containers`。

```toml
[dev-dependencies]
aster_forge_test = { git = "https://github.com/AsterCommunity/AsterForge", features = ["redis"] }
```

## 最小示例

```rust
use aster_forge_test::{redis::RedisTestContainer, suite::TestContainerSuite};
use std::sync::OnceLock;

fn test_suite() -> &'static TestContainerSuite {
    static SUITE: OnceLock<TestContainerSuite> = OnceLock::new();
    SUITE.get_or_init(|| TestContainerSuite::new("myproduct-cache"))
}

#[tokio::test]
async fn works_against_real_redis() {
    let container = RedisTestContainer::start(test_suite()).await;
    let client = redis::Client::open(container.url()).unwrap();
    // ... 用真实连接跑断言
}
```

PostgreSQL 测试应让 Forge 持有隔离库生命周期，产品只运行自己的 migration 和 seed：

```rust
let postgres = PostgresTestContainer::start(test_suite()).await;
let database = postgres.create_database("myproduct_case_123").await;
let connection = database.connect().await;
// ... product migration and assertions
connection.close().await.unwrap();
database.cleanup().await;
```

需要复用已经完成 migration/seed 的模板库时，模板内容仍由产品负责，Forge 只负责安全建库、登记和清理：

```rust
let database = postgres
    .create_database_from_template("myproduct_case_456", "myproduct_template")
    .await;
```

跨进程复用 migration template 时，先由产品在 `SuiteFixtureLock` 内调用
`create_shared_database()`、迁移并发布 `SuiteFixtureState`；普通测试继续用
`create_database_from_template()` 创建进程所有的隔离库。MySQL 对应提供
`create_shared_database()` / `drop_shared_database()`，但产品仍负责账号、migration 和 schema
snapshot。

## 容器复用与隔离

- 容器命名：`aster-test-{suite}-{instance}-{service}`。`instance` 是当前 checkout 路径的 hash，所以同一机器上多个 worktree 各自持有独立容器，互不干扰。
- `ReuseDirective::Always`：同一 checkout 的多次测试运行复用同一容器。**容器数据在运行之间保留**，测试 key / 数据库名必须带进程唯一前缀（例如 pid + 自增计数）。
- 容器镜像 tag 固定（redis:7-alpine、postgres:16、mysql:8.4），升级 tag 是有意识的变更。
- MySQL helper 会在每次启动或复用时配置足够大的 `table_definition_cache`，避免大型测试二进制并行创建隔离 schema 时耗尽 prepared statement 的自动 reprepare 次数。
- MySQL helper 同时配置 `max_connections`，覆盖 nextest process-per-test 下每个测试进程的 writer/reader pool；这两个值都属于 endpoint identity，配置契约变化会强制重新探测并更新 reusable container。

## 状态机与错误边界

`state` 模块在 `std::env::temp_dir()/aster-testcontainers-{suite}/` 下维护 per-service 的状态文件：

- `ContainerStateLock`：fs2 独占文件锁，保护 read-modify-write 周期。
- `SharedContainerState`：登记存活测试进程 PID 和它们创建的资源名（如 per-test 数据库）。
- `SharedContainerEndpoint`：保存 reusable container 最近一次通过探针的 image/container identity 与 host port；新测试进程先直接探测该 endpoint，失效时才重新走 Docker attach/start。
- `ContainerLease`：Drop 时 prune 已退出进程的条目。测试进程异常退出时，下一次运行的 `start()` 也会 prune，孤儿资源最终会被回收。
- nextest 子进程按 `NEXTEST_RUN_ID` 登记 execution ownership；同一 run 内已经退出的进程资源延迟保留，避免数据库 suite 在建 schema 的同时持续 drop 前序 schema。下一次 run 会用新的 run id 一次性识别并回收这些资源。
- `SuiteFixtureLock` / `SuiteFixtureState`：用于跨 nextest 进程复用产品拥有的迁移 template 或 schema snapshot。它保存 fixture/backend identity、container identity、migration/schema fingerprint、resource 和 producer version，并以临时文件 + rename 发布完整状态；产品在持锁期间验证或重建 fixture。
- PostgreSQL 孤儿库在删除前会转记到当前测试进程；即使回收过程再次中断，下一次运行仍能继续清理。
- 共享容器的 `start()` 只在真实服务探针通过后返回：PostgreSQL/MySQL 使用数据库连接，Redis 完成认证并执行 `PING`，Mailpit 同时检查 SMTP 与 HTTP API；不把镜像日志文案或仅建立 TCP 连接当作 readiness 契约。

边界说明：

- 状态文件损坏或锁失败会直接 panic。这是测试设施，不是生产服务。
- 非 Unix 平台没有 `kill -0`，保守地假设进程存活，即孤儿资源不会被清理（功能正确，只是清理变懒）。
- `PostgresTestContainer::create_database()` / `create_database_from_template()` 返回 `PostgresTestDatabase`，统一负责安全建库、连接重试、孤儿库清理和显式销毁。产品侧只负责数据库名称、模板内容、migration 与 seed 数据。
- `remember_shared_resource()` / `forget_shared_resource()` 用于登记或移除 suite-scoped 的产品 fixture。它们不会随 producer PID 退出而进入 stale cleanup；产品必须以 fingerprint 驱动的协议显式清理 superseded resource。
- MySQL helper 当前只提供 root 连接 URL；涉及产品用户授权时仍由产品测试 harness 负责，避免 Forge 猜测产品账号模型。
- `SmtpTestContainer` 封装 Mailpit 的 SMTP/API 端口和消息 API。`start()` 会同时等待 SMTP 和 HTTP API ready，返回后可以直接调用 `clear_messages()` / `message_count()`；产品测试只负责邮件业务配置与投递断言，不重复拼 Mailpit URL、启动重试或解析其 JSON。

## 真实子进程

`process::TestProcess` 为每个服务进程创建独立临时工作目录，将 stdout/stderr 写入日志文件，并在进程异常退出时把日志尾部放进 panic。`Drop` 会终止仍在运行的子进程并删除临时目录。产品侧仍负责命令参数、环境变量、HTTP readiness 和业务断言。

Unix 平台可使用 `terminate_gracefully(timeout)` 发送 `SIGTERM` 并等待进程退出，用于验证真实 shutdown drain 和 lease release；超时后由调用方决定如何报告，再由 `Drop` 兜底终止进程。

```rust
use aster_forge_test::process::{TestProcess, available_loopback_port};
use std::process::Command;

let port = available_loopback_port();
let mut command = Command::new(env!("CARGO_BIN_EXE_my_service"));
command.env("APP_PORT", port.to_string());
let mut process = TestProcess::spawn("primary-a", &mut command);
process.assert_running();
```

## 测试要求

- crate 自身的单元测试覆盖状态机（注册、prune、锁文件 JSON round-trip、lease 清理）和 `wait_until`。
- 消费方 crate 负责自己服务语义的集成测试，参考 `aster_forge_cache/tests/redis_container.rs`。

## 参考项目

- AsterDrive：`tests/common` 是这套机制的原始实现来源。
- AsterYggdrasil：同样形态的共享容器状态机，验证了抽象可以跨产品复用。
