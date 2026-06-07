# 模板生成

AsterForge 可以作为 `cargo-generate` starter repository，也可以直接复制仓库后运行 `./init.sh` 初始化命名。

## 使用 cargo-generate

安装：

```bash
cargo install cargo-generate
```

生成新项目：

```bash
cargo generate --git https://github.com/AsterCommunity/AsterForge --name my-service
cd my-service
./init.sh
```

`cargo-generate.toml` 会过滤常见本地产物，例如：

- `target/`
- `data/`
- `tmp/`
- `coverage/`
- `frontend-panel/node_modules/`
- `frontend-panel/dist/`
- `frontend-panel/test-results/`
- `docs/node_modules/`
- `docs/.vitepress/dist/`
- `docs/.vitepress/cache/`

项目级工具配置会随模板保留，包括 `.gcop`。这类配置属于新项目也需要继承的工程工作流，不应该被本地初始化误删。

## 使用 init.sh

`./init.sh` 会替换项目名、crate 名、slug、环境变量 token 和仓库地址。

查看参数：

```bash
./init.sh --help
```

非交互示例：

```bash
./init.sh \
  --name MyService \
  --crate my_service \
  --slug myservice \
  --upper MYSERVICE \
  --repo https://github.com/AsterCommunity/MyService \
  --yes
```

Dry run：

```bash
./init.sh --name MyService --dry-run
```

脚本会跳过构建产物、运行时数据、前端依赖、测试输出和文档构建缓存。它只处理文本文件，并且只在检测到旧标识时替换。

## 命名规则

推荐保持这些名字一致：

```text
Display name: MyService
Rust crate:   my_service
Flat slug:    myservice
Kebab slug:   my-service
Upper token:  MYSERVICE
```

`init.sh` 会从 display name 自动推导默认值，但正式项目建议显式传参，避免路径名或临时目录名影响结果。

## 初始化后检查

初始化后建议执行：

```bash
cargo fmt
cargo generate-lockfile
cargo check --bins
cd frontend-panel
bun install
bun run check
```

如果你改了 OpenAPI：

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

如果你改了 docs：

```bash
cd docs
bun install
bun run docs:build
```

## 需要人工确认的内容

模板初始化只能处理通用命名，不能替你决定产品语义。生成后至少检查：

- README 和 docs 是否需要补产品说明。
- `config.example.toml` 默认值是否合适。
- Docker image 名称和 GitHub Actions 发布目标是否正确。
- Admin UI 的 branding 是否符合新项目。
- 新增业务 API 是否有 OpenAPI 注解。
- 新增管理员操作是否写 audit log。
- 新增后台任务是否有 presentation 和清理策略。
