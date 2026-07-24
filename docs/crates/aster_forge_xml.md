# aster_forge_xml

`aster_forge_xml` 是基于 `quick-xml` 的高性能 XML 树结构库，提供与 `xmltree` 功能对等的 API，性能显著提升（预期 **3–8×**）。

## 设计目标

- **功能对等**：与 `xmltree::Element` 相同的 API 集
- **高性能**：利用 `quick-xml` 的零拷贝流式解析
- **安全优先**：内置 5 重安全检查（深度/元素数/输入大小/DTD/ENTITY）

## 适用场景

- 中小型 XML 文档的解析与序列化（<10 MB）
- 需要 DOM-like 树操作（遍历、查询、修改）
- xmltree 的替代升级方案
- 需要安全解析（防 XML 炸弹、DTD 攻击）

不适合的场景：

- **超大 XML（>100 MB）**：DOM 模型占用过多内存，建议使用 SAX/StAX 流式解析
- **仅需简单读取**：直接使用 `quick-xml` 的 Reader/Writer 更轻量

## Cargo 接入

```toml
[dependencies]
aster_forge_xml = { git = "https://github.com/AsterCommunity/AsterForge" }
```

## 快速开始

```rust
use aster_forge_xml::Element;

// 解析
let xml = r#"<root><item id="1">hello</item></root>"#;
let elem = Element::from_str(xml)?;

// 查询
assert_eq!(elem.name, "root");
assert_eq!(elem.get_child("item").unwrap().get_text(), Some("hello"));
assert_eq!(elem.get_child("item").unwrap().get_attr("id"), Some("1"));

// 修改
elem.get_child_mut("item").unwrap().set_text("world");
elem.set_attr("version", "2.0");

// 序列化
let output = elem.to_string();
elem.write(std::io::stdout())?;
```

## API 概览

### 解析

| 方法 | 说明 |
|------|------|
| `Element::from_str(xml)` | 从字符串解析 |
| `Element::from_bytes(bytes)` | 从字节切片解析 |
| `Element::from_reader(reader, options)` | 从 Read 源解析（可配置） |
| `Element::from_file(path)` | 从文件解析 |

### 属性操作

| 方法 | 说明 |
|------|------|
| `get_attr(name)` / `set_attr(name, value)` | 获取/设置属性 |
| `has_attr(name)` / `remove_attr(name)` | 判断/移除属性 |
| `num_attrs()` / `iter_attrs()` | 属性数量/迭代 |
| `clear_attributes()` | 清空所有属性 |

### 子节点操作

| 方法 | 说明 |
|------|------|
| `push(child)` / `take_child(name)` | 追加/移除子元素 |
| `get_child(name)` / `get_child_mut(name)` | 按名称查找 |
| `get_children(name)` | 获取所有匹配子元素 |
| `has_children()` / `num_children()` | 子节点判断/计数 |
| `clear_children()` | 清空所有子元素 |

支持谓词：`get_child("name")` 或 `get_child(("name", "namespace"))`。

### 文本操作

| 方法 | 说明 |
|------|------|
| `get_text()` / `set_text(text)` | 获取/设置文本 |
| `take_text()` / `has_text()` | 取出/判断文本 |
| `clear_text()` | 清空文本 |

### 遍历与查找

| 方法 | 说明 |
|------|------|
| `descendants()` | 深度优先遍历迭代器 |
| `descendants_mut()` | 可变遍历（返回 Vec） |
| `find(path)` | 按路径查找（如 `"book/title"`） |
| `find_mut(path)` | 按路径查找（可变） |

### 构建器模式

```rust
let elem = Element::new("root")
    .with_attr("version", "1.0")
    .with_child(Element::new("child").with_text("data"))
    .with_namespace("urn:example");
```

### 序列化

| 方法 | 说明 |
|------|------|
| `elem.to_string()` / `Display` | 格式化输出（2 空格缩进） |
| `elem.write(writer)` | 写入 io::Write |
| `elem.write_with_config(writer, opts)` | 自定义格式 |

## 安全配置

```rust
use aster_forge_xml::ParseOptions;

let options = ParseOptions::new()
    .max_depth(64)           // 最大嵌套深度
    .max_elements(10_000)    // 最大元素数量
    .max_size(1024 * 1024)   // 最大输入大小（1 MB）
    .allow_dtd(false)        // 拒绝 DTD（默认）
    .allow_entity(false);    // 拒绝 ENTITY（默认）
```

## 序列化配置

```rust
use aster_forge_xml::SerializeOptions;

// 默认：2 空格缩进
let opts = SerializeOptions::default();

// 紧凑模式（一行输出）
let opts = SerializeOptions::new().no_indent();

// Tab 缩进
let opts = SerializeOptions::new().indent(b'\t', 1);
```

## 与 xmltree 对比

| 特性 | xmltree | aster_forge_xml |
|------|---------|----------------|
| 底层引擎 | xml-rs (pull-parser) | quick-xml (零拷贝) |
| 解析性能 | 基线 | **3–8× 提升** |
| 序列化性能 | 基线 | **2–5× 提升** |
| API 风格 | 字段直接访问 | 方法调用 + 构建器模式 |
| ElementPredicate | ✅ | ✅ |
| descendants / find | ❌ | ✅ |
| 构建器链式调用 | ❌ | ✅ |
| 输入大小限制 | ❌ | ✅ (默认 10 MB) |
| 深度限制 | ❌ | ✅ (默认 128) |
| 元素数量限制 | ❌ | ✅ (默认 100K) |
| DTD/ENTITY 拒绝 | ❌ | ✅ (默认拒绝) |
| 子元素存储 | `Vec<XMLNode>` 枚举 | `Vec<Element>` 独立文本/PI |

## 安全检查说明

解析器内置 5 层安全防线，全部默认启用：

```
输入字节流
  │
  ├─ max_size: 超出 10 MB → MaxSizeExceeded
  │
  ├─ max_depth: 超出 128 层 → MaxDepthExceeded
  │
  ├─ max_elements: 超出 100K → MaxElementsExceeded
  │
  ├─ allow_dtd: false → DtdNotAllowed
  │    └─ 防止 Billion Laughs 攻击
  │
  └─ allow_entity: false → EntityNotAllowed
       └─ 防止 ENTITY 展开
```

## 性能数据

简要数据（Windows, Ryzen, Rust 1.94）：

| 基准 | 时间 |
|------|------|
| 解析 3 元素 | 4.0 µs |
| 解析 25 元素 | 40.1 µs |
| 解析 505 元素 | 441.1 µs |
| 100 层嵌套 | 64.2 µs |
| 序列化 3 元素 | 1.6 µs |
| 序列化 505 元素 | 159.4 µs |
