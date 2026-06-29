import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";

// https://vite.dev/config/
export default defineConfig({
	plugins: [
		react(),
		tailwindcss(),
		VitePWA({
			registerType: "autoUpdate",
			injectRegister: "script",
			includeAssets: ["favicon.svg"],
			manifest: {
				name: "%ASTER_SERVICE_TITLE%",
				short_name: "%ASTER_SERVICE_TITLE%",
				description: "%ASTER_SERVICE_DESCRIPTION%",
				theme_color: "#0f172a",
				background_color: "#f8fafc",
				display: "standalone",
				icons: [
					{
						src: "/favicon.svg",
						sizes: "any",
						type: "image/svg+xml",
						purpose: "any",
					},
					{
						src: "/favicon.svg",
						sizes: "any",
						type: "image/svg+xml",
						purpose: "maskable",
					},
				],
			},
			workbox: {
				globPatterns: ["index.html", "assets/**/*.{js,css,mjs,woff2}"],
				navigateFallback: "index.html",
				navigateFallbackDenylist: [
					/^\/api\//,
					/^\/health(?:\/.*)?$/,
				],
			},
			devOptions: {
				enabled: true,
				navigateFallbackAllowlist: [/^\/$/],
			},
		}),
	],
	base: "/",
	resolve: {
		alias: {
			"@": path.resolve(__dirname, "./src"),
		},
		dedupe: ["react", "react-dom"],
	},
	server: {
		proxy: {
			"/api": "http://127.0.0.1:{{server_port}}",
			"/health": "http://127.0.0.1:{{server_port}}",
		},
	},
	build: {
		target: "esnext",
		outDir: "dist",
		emptyOutDir: true,
	},
});
