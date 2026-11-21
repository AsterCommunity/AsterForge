# Reference Projects

- [AsterDrive](https://github.com/AsterCommunity/AsterDrive): the larger product surface, useful for complete storage, external-auth, task, database, and metrics integration examples.
- [AsterYggdrasil](https://github.com/AsterCommunity/AsterYggdrasil): a lighter product boundary, useful for seeing which task, runtime, API, and middleware responsibilities stay in the product repository.

The Chinese documentation contains the complete crate-by-crate integration notes.

## AsterDrive

Repository: [AsterCommunity/AsterDrive](https://github.com/AsterCommunity/AsterDrive)

Useful as a reference for:

- Pagination, sorting and cursors in the file and folder APIs.
- Caching, database connections, runtime tasks and metric recording.
- External-auth connectors and dedicated provider configuration.
- Storage keys, S3-compatible configuration and object-storage boundaries.
- Complex business surroundings: file classification, upload, sharing, WebDAV, WOPI and more.

Drive carries the largest business surface, so complete Forge crate usage usually appears there first. Its complexity is also why Drive's business-layer abstractions must not be moved into Forge directly.

## AsterYggdrasil

Repository: [AsterCommunity/AsterYggdrasil](https://github.com/AsterCommunity/AsterYggdrasil)

Useful as a reference for:

- Actix middleware integration.
- Lightweight external-auth integration.
- Background-task integration with Forge lease, dispatch and runtime primitives.
- Service startup/shutdown wiring for the shared panic hook, logging, metrics and DB handles.
- OpenAPI-feature API schema and route annotations.

Yggdrasil keeps the integration boundary clearest, which makes it useful for seeing what a product should keep on its own side. In background tasks, for example, task kind, payload/result and the SeaORM repository stay in Yggdrasil, while lease, heartbeat and dispatch lifecycle live in Forge.
