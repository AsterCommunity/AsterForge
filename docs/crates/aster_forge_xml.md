# aster_forge_xml

`aster_forge_xml` 是 Aster 产品共用的有界 XML 内核，提供 source-backed arena DOM、namespace-aware streaming reader、选择性 subtree capture 和 direct streaming writer。它用 `quick-xml` 读取和写出事件，不会把每个 name、attribute、text 和 subtree 都复制成独立 `String`，也不会构造递归拥有子树的 DOM。

## 用途边界

Forge XML 负责：

- 用平坦 arena 保存 node，并用 `NodeId` 连接 parent、child 和 sibling。
- 普通 UTF-8 name、attribute、text 和 subtree 直接引用原始输入区间。
- entity 解码或显式 whitespace normalization 改变值时，才进入 owned value pool。
- 保留 mixed content、CDATA、comment、processing instruction 的顺序。
- 正确处理 default namespace、prefix shadowing、undeclaration 和 attribute namespace。
- 统一限制输入字节、深度、元素、属性、文本和事件数量。
- 默认拒绝 DTD、自定义 entity、多个 root、尾部垃圾和非法编码。
- 对任意 `BufRead` 做有界流式读取，并允许直接跳过或只保留选中的 subtree。
- 对任意 `Write` 直接生成 XML，并检查 document state、namespace binding、名称、字符、深度、属性数和输出字节上限。

产品侧仍负责 XML schema、协议字段、HTTP body 聚合上限、错误映射、权限和存储语义。已知 schema 的 WOPI/COS/WebDAV request 应优先使用 `XmlStreamReader` 做 typed/event parsing；需要随机查询时使用 `BorrowedDocument` 或 `OwnedDocument`；只需保留未知扩展节点时使用 `capture_current`；生成中大型 response 时使用 `XmlStreamWriter`。

## Cargo 接入

```toml
[dependencies]
aster_forge_xml = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## Borrowed document

```rust
use aster_forge_xml::BorrowedDocument;

let body = br#"<D:prop xmlns:D="DAV:"><D:displayname>a&amp;b</D:displayname></D:prop>"#;
let document = BorrowedDocument::parse(body.as_slice())?;
let display_name = document
    .root()
    .get_child_ns("displayname", "DAV:")
    .expect("fixture contains DAV:displayname");

assert_eq!(display_name.text().as_deref(), Some("a&b"));
assert_eq!(display_name.raw_xml(), br#"<D:displayname>a&amp;b</D:displayname>"#);
```

`BorrowedDocument<'a>` 不复制完整输入；document 的生命周期受输入 byte slice 约束。name、普通 attribute/text 和 raw subtree payload 直接引用 source span，只有 entity decoding 或 normalization 改变值时才进入 owned value pool；arena node、link、namespace scope 等索引元数据仍会分配，因此准确说法是 source-backed、payload zero-copy DOM，而不是“整个 DOM 零分配”。`raw_xml()` 返回 element 在原始输入中的精确 byte slice，适合 WebDAV dead property、LOCK owner 和未知扩展 subtree。

## Owned document 与 validated value

需要跨 task、cache 或 store 保留文档时使用 owned 形态：

```rust
use std::sync::Arc;
use aster_forge_xml::{OwnedDocument, ValidatedXml};

let source: Arc<[u8]> = Arc::from(br#"<owner><href>mailto:a@example.test</href></owner>"#.as_slice());
let document = OwnedDocument::parse(Arc::clone(&source))?;

let validated = ValidatedXml::new(source)?;
let cheap_clone = validated.clone();
assert_eq!(cheap_clone.as_bytes(), validated.as_bytes());
```

- `OwnedDocument = XmlDocument<Arc<[u8]>>`：arena 和 source 一起拥有。
- `OwnedDocument::from_reader`：只读取到配置上限再解析。
- `ValidatedXml`：`Arc<OwnedDocument>` 包装，clone 不复制 source 或 arena。
- `write_original`：写出完整原始文档，包括 declaration、root 外 comment/PI 和空白。

`write_original` 是 exact byte copy，不是结构化 serializer。生成 WebDAV/WOPI response 时应使用后面的 `XmlStreamWriter`，不要先构造完整 DOM。

## Streaming reader 与选择性 capture

```rust
use std::io::BufReader;
use aster_forge_xml::{XmlSafetyPolicy, XmlStreamEvent, XmlStreamReader};

let body = br#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/a</D:href></D:response></D:multistatus>"#;
let mut reader = XmlStreamReader::new(BufReader::new(body.as_slice()), XmlSafetyPolicy::untrusted())?;

loop {
    match reader.read_event()? {
        XmlStreamEvent::Start(start) if start.name()?.matches("href", Some("DAV:")) => {
            assert_eq!(reader.read_text_current()?, "/a");
        }
        XmlStreamEvent::Start(start) if start.name()?.matches("unknown", Some("urn:extension")) => {
            let retained = reader.capture_current(64 * 1024)?;
            assert_eq!(retained.document().root().namespace(), Some("urn:extension"));
        }
        XmlStreamEvent::Start(start) if start.name()?.matches("ignored", None) => reader.skip_current()?,
        XmlStreamEvent::Eof => break,
        _ => {}
    }
}
```

`XmlStreamReader` 复用 event buffer，只保留当前事件、namespace resolver 和与深度成正比的 namespace state，不保留完整 token 列表或 DOM。`capture_current(max_bytes)` 只 materialize 当前 subtree，并自动补齐祖先作用域内、该 subtree 独立解析所需的 namespace declaration；`skip_current()` 继续执行完整安全和 well-formedness 检查，但不会保留被跳过节点。

## Streaming writer

```rust
use aster_forge_xml::XmlStreamWriter;

let mut writer = XmlStreamWriter::new(Vec::new())?;
writer.start_element("D:multistatus", [("xmlns:D", "DAV:")])?;
writer.start("D:response")?;
writer.start("D:href")?;
writer.text("/files/a&b")?;
writer.end_element()?;
writer.end_element()?;
writer.end_element()?;
let output = writer.finish()?;

assert_eq!(output, br#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/files/a&amp;b</D:href></D:response></D:multistatus>"#);
```

`XmlStreamWriter<W: Write>` 直接把事件写到 socket、file、buffer 或 compression stream。`XmlWriteOptions` 控制 XML declaration、最大输出字节、最大深度和单元素最大属性数；writer 会检查 root lifecycle、prefix binding、namespace-expanded duplicate attribute、XML name/control character、CDATA/comment/PI 约束和 I/O failure。`validated_subtree` 可以嵌入一个自包含的 `ValidatedXml` root，并继续执行 writer depth、attribute 和 byte limit。

## 查询 API

- `root` / `node`：取得 root 或按稳定 `NodeId` 查询。
- `children` / `child_elements`：按文档顺序遍历直接子节点。
- `get_child` / `get_child_ns`：按 local name 和 namespace 查询直接子元素。
- `attributes` / `attribute` / `attribute_ns`：按 qualified name 或 namespace 查询属性。
- `parent`：访问 parent element。
- `descendants`：非递归深度优先遍历，包含当前元素。
- `text`：拼接直接 Text 和 CDATA；单节点返回 borrowed `Cow`。
- `raw_xml`：返回精确原始 subtree bytes。

tree 是 immutable view。需要修改业务字段时，应解析成 typed DTO；需要输出新文档时，应走 event writer，而不是复制 arena 生成另一棵可变递归树。

## 安全策略

```rust
use aster_forge_xml::{BorrowedDocument, ParseOptions, XmlSafetyPolicy, validate_xml_input};

let policy = XmlSafetyPolicy {
    max_input_bytes: 1024 * 1024,
    max_depth: 64,
    max_elements: 10_000,
    max_attributes_per_element: 128,
    max_text_bytes: 512 * 1024,
    max_events: 100_000,
    reject_doctype: true,
};

validate_xml_input(body, policy)?; // event-only，不构建 DOM
let document = BorrowedDocument::parse_with_options(
    body,
    &ParseOptions::new().safety_policy(policy),
)?;
```

如果只需要分派 root：

```rust
use aster_forge_xml::{XmlSafetyPolicy, xml_root_local_name};

let method = xml_root_local_name(body, XmlSafetyPolicy::untrusted())?;
```

`XmlSafetyError` 保留这些可映射分类：`InvalidPolicy`、`InputTooLarge`、`OutputTooLarge`、`ExternalEntity`、`TooDeep`、`TooManyElements`、`TooManyAttributes`、`TextTooLarge`、`TooManyEvents`、`InvalidEncoding`、`Malformed`。

## 错误边界

- `Error::Safety`：输入越过 `XmlSafetyPolicy`。
- `Error::InvalidXml`：底层 XML 结构错误。
- `Error::InvalidData`：writer state、name、namespace 或 XML value 不合法。
- `Error::Io`：reader/writer 的底层 I/O 失败。

产品 API 层负责映射成 WebDAV/WOPI/对象存储协议响应和面向用户的文案。

## 测试要求

- plain value 必须落在 source buffer 内，且 owned value pool 为空。
- entity/normalization 只为发生变化的值分配。
- mixed content、所有 node kind、parent/sibling link 和 text join。
- default namespace、shadowing、undeclaration、namespaced attribute。
- DTD/entity、多个 root、尾部垃圾、非法 UTF-8 和每一种 limit 边界。
- WebDAV dead property / LOCK owner subtree exact bytes。
- 20,000 层 parse、traversal 和 drop 不依赖递归 destructor。
- 100,000 response streaming walk 和 selective capture 不构造整棵 DOM。
- streaming writer 覆盖 namespace、escaping、所有 node kind、document lifecycle、limit 精确边界、I/O failure、subtree embedding 和 25,000-response direct generation。
- 与 `xmltree` 对照 supported parse contract 和 round-trip。
- 与 `roxmltree` 对照 source-backed tree、namespace、attribute、text、child query 和 exact source range，并明确 CDATA/default namespace empty-URI 的 node model 差异。
- `proptest` 每次生成 256 组有界随机树，执行 writer → validator → Forge DOM → `roxmltree` round-trip；随机 byte input 同时喂给 validator、arena parser 和 stream reader，检查资源边界内不 panic。
- borrowed arena、owned validated document、event walk 和 writer 的 allocation/heap/RSS probe。

长期 fuzz harness 位于 `crates/aster_forge_xml/fuzz`，将 parser/stream/capture 与 writer rejection/round-trip 分成两个 target：

```bash
cargo fuzz run parse_stream
cargo fuzz run writer_roundtrip
```

## Benchmark

`xmltree`、`roxmltree` 和 `proptest` 只在本 crate 的 dev-dependencies 中，用于行为对照、property test 和 benchmark，不进入正式依赖图；`libfuzzer-sys` 只存在于独立的 `fuzz` workspace。

```bash
cargo bench -p aster_forge_xml --bench xml_bench
cargo bench -p aster_forge_xml --bench xml_memory
```

CPU benchmark 固定 `PROPFIND`、WOPI discovery 和 1,000-response `multistatus` workload，比较：

- `forge_arena`
- `roxmltree_borrowed`
- `forge_stream_reader`
- `forge_stream_validation`（执行完整 stream safety/namespace/value validation，但不读取 event field）
- `xmltree_owned`
- `validate_plus_xmltree`
- `quick_xml_ns_buffered_decoded`（buffered `NsReader`、namespace resolution、attribute normalization、text decode/unescape 和 end-name processing）
- `quick_xml_borrowed_events` lower bound
- `arena_original` 与 `xmltree_compact`（两者语义不同，禁止用这组结果宣称 serializer 倍率）
- `forge_stream_writer`、`quick_xml_writer` lower bound 与 `xmltree_build_and_write`（同一个 1,000-response 业务生成 workload）

内存 probe 让每个 implementation/fixture 在独立子进程运行，并输出 allocation count、累计申请字节、peak live heap、retained heap 和 peak RSS；最大 fixture 是 10,000-response `multistatus`。reader 输入在 measurement 前创建；writer 同时测量保留完整 output `Vec` 和写入 `sink` 两种形态，后者用于确认 writer 自身 retained heap 与文档大小无关。

2026-07-25 在 `review/pr-3` working tree（base `305ef17343fc`）、`rustc 1.97.1`、`aarch64-apple-darwin` 的 release benchmark 样本中，arena parse 相对 `xmltree_owned` 的 Criterion central estimate 分别是 `2.9×`（PROPFIND）、`2.9×`（WOPI）和 `7.8×`（multistatus 1000），因此“约 3–8×”在这组固定 workload 上有实测支持；`forge_stream_writer` 生成 multistatus 1000 为 `1.034 ms`，`xmltree_build_and_write` 为 `3.326 ms`，即 `3.22×`，落在“2–5×”范围内。这些是 workload-specific evidence，不是跨机器常量，也不应用 `arena_original` exact copy 与 serializer 的差值替代。

同一轮 2,625,862-byte multistatus 10000 memory probe 中，`forge_arena_borrowed` retained heap 为 11,140,124 bytes，`forge_validated_owned` 为 13,766,124 bytes，`xmltree_owned` 为 111,004,222 bytes；优化后的 `forge_stream_reader` peak live heap 为 404 bytes、retained heap 为 0。direct writer 写入 `sink` 时 peak live heap 为 606 bytes、retained heap 为 0；写入 output `Vec` 时 retained capacity 为 3,801,088 bytes；`xmltree_build_and_write` peak live heap 为 65,039,004 bytes。RSS 包含进程、allocator 和预先创建的 input，只能比较同一轮独立子进程结果，不能当作 retained DOM bytes。

加入真正的 source-backed DOM 对照后，`roxmltree_borrowed` 在三个 parse workload 上分别比 Forge arena 快约 `1.5×`、`1.5×` 和 `1.7×`；multistatus 10000 retained heap 为 9,680,148 bytes，Forge arena 为 11,140,124 bytes。Forge 为 namespace persistent scope、所有 node kind 的独立保留、owned value pool、统一 safety classification 和 exact document/subtree API 付出了约 15% retained heap 与 1.5–1.7× CPU 成本；大文档只做顺序处理时仍应选择 `XmlStreamReader`，不应为了 API 统一强行构建 arena。

`quick_xml_borrowed_events` 只是 slice-specialized tokenizer 下界，没有执行 Forge 的 namespace、完整 value decode、single-root 和资源限制，不能直接用来判断 wrapper 开销。加入工作量更接近的 `quick_xml_ns_buffered_decoded` 后，当前样本中 `forge_stream_validation` 约为其 `1.0–1.4×`，读取 name/attribute/text 的完整 `forge_stream_reader` 约为其 `1.5–1.9×`；剩余差值主要来自安全计数、root/depth state，以及 attribute name/namespace 的验证后再次访问。stream hot path 已去掉每个 Start event 的备用 raw copy，复用已验证的 owned attribute value，并让 Text/CDATA/Comment event 直接携带首次 decode 结果；固定 2,000 次 multistatus 1000 walk 的交替 A/B 样本中，优化前后中位数为 `4.968 s` 与 `4.160 s`，约快 `16%`。WOPI allocation probe 从 1,016 次、59,921 bytes 降至 767 次、46,549 bytes；multistatus stream allocation 从 15 次、738 bytes 降至 14 次、710 bytes。这些同样是本机 workload-specific evidence。

## 参考项目

- AsterDrive：WOPI discovery、WebDAV properties/LOCK owner、Tencent COS CORS/media metadata。
- `xmltree-rs`：行为与 benchmark 对照，不进入生产依赖。
- `roxmltree`：source-backed read-only tree 的性能与内存基线，不进入生产依赖。
