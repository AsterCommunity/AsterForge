import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import { withMermaid } from "vitepress-plugin-mermaid";

const __dirname = dirname(fileURLToPath(import.meta.url));

function getVersion(): string {
	try {
		const cargoPath = resolve(__dirname, "../../Cargo.toml");
		const content = readFileSync(cargoPath, "utf-8");
		const match = content.match(/^version\s*=\s*"([^"]+)"/m);
		return match ? match[1] : "unknown";
	} catch {
		return "unknown";
	}
}

const version = getVersion();

export default withMermaid(
	defineConfig({
		title: "AsterForge",
		description:
			"Reusable Rust + React service foundation for Aster projects.",
		lang: "zh-CN",
		cleanUrls: true,
		lastUpdated: true,
		head: [
			["meta", { name: "theme-color", content: "#111827" }],
			["link", { rel: "icon", href: "/favicon.svg" }],
		],
		locales: {
			root: {
				label: "简体中文",
				lang: "zh-CN",
				title: "AsterForge",
				description: "Aster 项目的可复用 Rust + React 服务地基。",
				themeConfig: {
					outline: {
						label: "本页内容",
					},
					lastUpdated: {
						text: "最后更新",
					},
					docFooter: {
						prev: "上一页",
						next: "下一页",
					},
					darkModeSwitchLabel: "外观",
					darkModeSwitchTitle: "切换到深色主题",
					lightModeSwitchTitle: "切换到浅色主题",
					sidebarMenuLabel: "菜单",
					returnToTopLabel: "返回顶部",
					langMenuLabel: "切换语言",
					skipToContentLabel: "跳到内容",
					search: {
						provider: "local",
						options: {
							translations: {
								button: {
									buttonText: "搜索",
									buttonAriaLabel: "搜索",
								},
								modal: {
									displayDetails: "显示详细列表",
									resetButtonTitle: "清除搜索",
									backButtonTitle: "关闭搜索",
									noResultsText: "没有找到相关结果",
									footer: {
										selectText: "选择",
										selectKeyAriaLabel: "Enter",
										navigateText: "切换",
										navigateUpKeyAriaLabel: "向上",
										navigateDownKeyAriaLabel: "向下",
										closeText: "关闭",
										closeKeyAriaLabel: "Escape",
									},
								},
							},
						},
					},
					nav: [
						{ text: "首页", link: "/" },
						{ text: "使用指南", link: "/guide/getting-started" },
						{ text: "部署", link: "/deployment/docker" },
						{ text: `v${version}`, link: "https://github.com/AsterCommunity/AsterForge" },
					],
					sidebar: [
						{
							text: "开始",
							items: [
								{ text: "概览", link: "/" },
								{ text: "快速开始", link: "/guide/getting-started" },
								{ text: "配置模型", link: "/guide/configuration" },
								{ text: "运行时", link: "/guide/runtime" },
								{ text: "认证", link: "/guide/authentication" },
								{ text: "审计与后台任务", link: "/guide/audit-tasks" },
								{ text: "模板生成", link: "/guide/template-generation" },
							],
						},
						{
							text: "部署",
							items: [{ text: "Docker", link: "/deployment/docker" }],
						},
					],
				},
			},
			en: {
				label: "English",
				lang: "en-US",
				title: "AsterForge",
				description:
					"Reusable Rust + React service foundation for Aster projects.",
				themeConfig: {
					nav: [
						{ text: "Home", link: "/en/" },
						{ text: "Guide", link: "/en/guide/getting-started" },
						{ text: "Deployment", link: "/en/deployment/docker" },
						{ text: `v${version}`, link: "https://github.com/AsterCommunity/AsterForge" },
					],
					sidebar: [
						{
							text: "Start",
							items: [
								{ text: "Overview", link: "/en/" },
								{ text: "Getting Started", link: "/en/guide/getting-started" },
								{ text: "Configuration", link: "/en/guide/configuration" },
								{ text: "Runtime", link: "/en/guide/runtime" },
								{ text: "Authentication", link: "/en/guide/authentication" },
								{ text: "Audit and Tasks", link: "/en/guide/audit-tasks" },
								{ text: "Template Generation", link: "/en/guide/template-generation" },
							],
						},
						{
							text: "Deployment",
							items: [{ text: "Docker", link: "/en/deployment/docker" }],
						},
					],
				},
			},
		},
		themeConfig: {
			search: {
				provider: "local",
			},
			socialLinks: [
				{ icon: "github", link: "https://github.com/AsterCommunity/AsterForge" },
			],
			footer: {
				message: "Released under the MIT License.",
				copyright: "Copyright (c) AptS-1547",
			},
		},
	}),
);
