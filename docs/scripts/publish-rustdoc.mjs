import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { execFileSync } from "node:child_process"

const __dirname = dirname(fileURLToPath(import.meta.url))
const docsRoot = resolve(__dirname, "..")
const repoRoot = resolve(docsRoot, "..")
const rustdocTarget = join(repoRoot, "target", "rustdoc")
const rustdocOutput = join(rustdocTarget, "doc")
const rustdocDist = join(docsRoot, ".vitepress", "dist", "crates", "rustdoc")
const rustdocCleanUrlEntry = join(docsRoot, ".vitepress", "dist", "crates", "rustdoc.html")

rmSync(rustdocOutput, { recursive: true, force: true })
execFileSync(
  "cargo",
  ["doc", "--workspace", "--all-features", "--no-deps", "--target-dir", rustdocTarget],
  { cwd: repoRoot, stdio: "inherit" },
)

rmSync(rustdocDist, { recursive: true, force: true })
mkdirSync(rustdocDist, { recursive: true })

const forgeCrates = []

for (const entry of readdirSync(rustdocOutput, { withFileTypes: true })) {
  const source = join(rustdocOutput, entry.name)
  const destination = join(rustdocDist, entry.name)

  if (entry.isFile()) {
    cpSync(source, destination)
    continue
  }

  if (
    entry.isDirectory() &&
    (entry.name.startsWith("aster_forge_") ||
      entry.name === "src" ||
      entry.name === "static.files")
  ) {
    cpSync(source, destination, { recursive: true })

    if (entry.name.startsWith("aster_forge_")) {
      forgeCrates.push(entry.name)
    }
  }
}

forgeCrates.sort()

const rustdocIndex = `<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>AsterForge Rust API documentation</title>
    <style>
      body {
        color: #1f2937;
        font: 16px/1.6 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        margin: 0;
      }
      main {
        margin: 0 auto;
        max-width: 960px;
        padding: 48px 24px;
      }
      h1 {
        font-size: 28px;
        line-height: 1.2;
        margin: 0 0 8px;
      }
      p {
        color: #4b5563;
        margin: 0 0 24px;
      }
      ul {
        display: grid;
        gap: 10px 24px;
        grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
        list-style: none;
        margin: 0;
        padding: 0;
      }
      a {
        border: 1px solid #d1d5db;
        border-radius: 8px;
        color: #0f766e;
        display: block;
        padding: 10px 12px;
        text-decoration: none;
      }
      a:hover {
        border-color: #0f766e;
        background: #f0fdfa;
      }
    </style>
  </head>
  <body>
    <main>
      <h1>AsterForge Rust API documentation</h1>
      <p>Generated rustdoc for workspace crates.</p>
      <ul>
${forgeCrates.map((crateName) => `        <li><a href="/crates/rustdoc/${crateName}/">${crateName}</a></li>`).join("\n")}
      </ul>
    </main>
  </body>
</html>
`

writeFileSync(join(rustdocDist, "index.html"), rustdocIndex)
writeFileSync(rustdocCleanUrlEntry, rustdocIndex)

if (!existsSync(join(rustdocDist, "index.html"))) {
  throw new Error("rustdoc index was not published")
}

if (!existsSync(rustdocCleanUrlEntry)) {
  throw new Error("rustdoc clean URL entry was not published")
}
