# 参考项目

Forge 文档会频繁引用 AsterDrive 和 AsterYggdrasil，因为这两个项目是当前共享 crate 的主要来源和首批接入方。

## AsterDrive

仓库链接：[AsterCommunity/AsterDrive](https://github.com/AsterCommunity/AsterDrive)

适合作为参考的场景：

- 文件与文件夹 API 的分页、排序和 cursor。
- 缓存、数据库连接、运行时任务和指标记录。
- 外部认证连接器和专用 provider 配置。
- 存储 key、S3 兼容配置和对象存储边界。
- 文件类型分类、上传、分享、WebDAV、WOPI 等复杂业务周边。

Drive 的特点是业务面更大，很多 Forge crate 的完整用法会先在 Drive 里出现。但也因为它更复杂，不能把 Drive 的业务层抽象直接搬进 Forge。

## AsterYggdrasil

仓库链接：[AsterCommunity/AsterYggdrasil](https://github.com/AsterCommunity/AsterYggdrasil)

适合作为参考的场景：

- Actix middleware 接入。
- 外部认证的较轻集成。
- 后台任务系统接入 Forge lease、dispatch、runtime primitives。
- shared panic hook、logging、metrics、DB handles 的服务启动/关闭链路。
- OpenAPI feature 下的 API schema 和 route annotation。

Yggdrasil 的特点是接入边界更清楚，适合看“产品侧应该保留什么”。例如后台任务里，task kind、payload/result、SeaORM repository 留在 Yggdrasil，lease、heartbeat、dispatch lifecycle 放到 Forge。
