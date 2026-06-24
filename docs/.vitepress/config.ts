import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { defineConfig } from "vitepress"
import { withMermaid } from "vitepress-plugin-mermaid"

const __dirname = dirname(fileURLToPath(import.meta.url))
const SITE_URL = "https://forge.astercosm.com/"
const ZH_SITE_DESCRIPTION =
  "AsterForge 开发文档，覆盖共享 Rust crates 的模块边界、接入方式、功能开关、测试要求和参考项目。"
const EN_SITE_DESCRIPTION =
  "AsterForge developer documentation for shared Rust crates, integration boundaries, feature flags, testing expectations, and reference projects."

function getVersion(): string {
  try {
    const cargoPath = resolve(__dirname, "../../Cargo.toml")
    const content = readFileSync(cargoPath, "utf-8")
    const match = content.match(/^version\s*=\s*"([^"]+)"/m)
    return match ? match[1] : "workspace"
  } catch {
    return "workspace"
  }
}

const version = getVersion()

const cratePages = [
  ["Actix middleware", "/crates/aster_forge_actix_middleware"],
  ["Alloc", "/crates/aster_forge_alloc"],
  ["API", "/crates/aster_forge_api"],
  ["API docs macros", "/crates/aster_forge_api_docs_macros"],
  ["Cache", "/crates/aster_forge_cache"],
  ["Crypto", "/crates/aster_forge_crypto"],
  ["Database", "/crates/aster_forge_db"],
  ["External auth", "/crates/aster_forge_external_auth"],
  ["File classification", "/crates/aster_forge_file_classification"],
  ["Logging", "/crates/aster_forge_logging"],
  ["Metrics", "/crates/aster_forge_metrics"],
  ["Panic", "/crates/aster_forge_panic"],
  ["Storage core", "/crates/aster_forge_storage_core"],
  ["Tasks", "/crates/aster_forge_tasks"],
  ["Utils", "/crates/aster_forge_utils"],
  ["Validation", "/crates/aster_forge_validation"],
] as const

function crateSidebar() {
  return [
    {
      text: "开始",
      items: [
        { text: "总览", link: "/guide/" },
        { text: "接入原则", link: "/guide/integration-principles" },
        { text: "参考项目", link: "/guide/reference-projects" },
      ],
    },
    {
      text: "Crates",
      items: cratePages.map(([text, link]) => ({ text, link })),
    },
  ]
}

export default withMermaid(
  defineConfig({
    title: "AsterForge",
    description: ZH_SITE_DESCRIPTION,
    lang: "zh-CN",
    cleanUrls: true,
    lastUpdated: true,
    sitemap: {
      hostname: SITE_URL,
    },
    head: [
      ["meta", { name: "theme-color", content: "#0f766e" }],
      ["link", { rel: "icon", type: "image/svg+xml", href: "/favicon.svg" }],
      ["meta", { property: "og:type", content: "website" }],
      ["meta", { property: "og:site_name", content: "AsterForge" }],
      ["meta", { name: "twitter:card", content: "summary" }],
    ],
    themeConfig: {
      logo: "/favicon.svg",
      outline: {
        label: "本页内容",
        level: [2, 3],
      },
      nav: [
        { text: "首页", link: "/" },
        { text: "总览", link: "/guide/" },
        { text: "模块", link: "/crates/aster_forge_actix_middleware" },
        { text: `版本 ${version}`, link: "https://github.com/AsterCommunity/AsterForge" },
      ],
      sidebar: crateSidebar(),
      socialLinks: [
        { icon: "github", link: "https://github.com/AsterCommunity/AsterForge" },
      ],
      search: {
        provider: "local",
      },
      footer: {
        message: "Shared crates for Aster services.",
        copyright: "MIT Licensed.",
      },
    },
    locales: {
      root: {
        label: "简体中文",
        lang: "zh-CN",
        title: "AsterForge",
        description: ZH_SITE_DESCRIPTION,
      },
      en: {
        label: "English",
        lang: "en-US",
        title: "AsterForge",
        description: EN_SITE_DESCRIPTION,
        themeConfig: {
          nav: [
            { text: "Home", link: "/en/" },
            { text: "Guide", link: "/en/guide/" },
            { text: "Chinese crate docs", link: "/crates/aster_forge_actix_middleware" },
          ],
          sidebar: [
            {
              text: "Guide",
              items: [
                { text: "Overview", link: "/en/guide/" },
                { text: "Reference projects", link: "/en/guide/reference-projects" },
              ],
            },
          ],
          outline: {
            label: "On this page",
            level: [2, 3],
          },
        },
      },
    },
  }),
)
