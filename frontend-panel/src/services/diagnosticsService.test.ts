import { describe, expect, it, vi } from "vitest";
import { ApiError } from "./http";

const apiMock = vi.hoisted(() => {
	const get = vi.fn();
	const rootGet = vi.fn();
	return {
		get,
		rootGet,
	};
});

vi.mock("./http", async () => {
	const actual = await vi.importActual<typeof import("./http")>("./http");
	return {
		...actual,
		api: {
			get: apiMock.get,
			root: {
				get: apiMock.rootGet,
			},
		},
	};
});

describe("diagnosticsService", () => {
	it("creates idle rows for registered endpoints", async () => {
		const { createIdleDiagnostics } = await import("./diagnosticsService");

		const rows = createIdleDiagnostics();

		expect(rows).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					id: "health",
					path: "/health",
					status: "idle",
					value: "not checked",
				}),
				expect.objectContaining({
					id: "system",
					path: "/api/v1/system/info",
					status: "idle",
				}),
				expect.objectContaining({
					id: "protected-example",
					path: "/api/v1/examples/protected",
					status: "idle",
				}),
			]),
		);
	});

	it("loads registered public APIs and marks authenticated calls as guarded", async () => {
		const controller = new AbortController();
		apiMock.rootGet.mockImplementation((path: string) => {
			if (path === "/health") {
				return Promise.resolve({
					status: "ok",
					version: "0.1.0",
					build_time: "test-build",
				});
			}
			if (path === "/health/ready") {
				return Promise.resolve({
					status: "ready",
					version: "0.1.0",
					build_time: "test-build",
				});
			}
			throw new Error(`unexpected root path ${path}`);
		});
		apiMock.get.mockImplementation((path: string) => {
			if (path === "/system/info") {
				return Promise.resolve({
					name: "AsterForge",
					version: "0.1.0",
					build_time: "test-build",
					site_title: "AsterForge",
				});
			}
			if (path === "/auth/check") {
				return Promise.resolve({ initialized: false });
			}
			if (path === "/external-auth/providers") {
				return Promise.resolve([]);
			}
			if (path === "/examples/public") {
				return Promise.resolve({
					message: "public example",
					build_time: "test-build",
				});
			}
			if (path === "/examples/protected") {
				return Promise.reject(
					new ApiError("auth.token_invalid", "missing session cookie"),
				);
			}
			throw new Error(`unexpected api path ${path}`);
		});

		const { loadServiceDiagnostics } = await import("./diagnosticsService");

		const rows = await loadServiceDiagnostics(controller.signal);

		expect(apiMock.rootGet).toHaveBeenCalledWith("/health", {
			signal: controller.signal,
		});
		expect(apiMock.rootGet).toHaveBeenCalledWith("/health/ready", {
			signal: controller.signal,
		});
		expect(apiMock.get).toHaveBeenCalledWith("/system/info", {
			signal: controller.signal,
		});
		expect(apiMock.get).toHaveBeenCalledWith("/examples/protected", {
			signal: controller.signal,
		});
		expect(rows).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					id: "health",
					status: "ok",
					value: "ok",
					detail: "v0.1.0",
				}),
				expect.objectContaining({
					id: "auth-check",
					status: "ok",
					value: "setup required",
				}),
				expect.objectContaining({
					id: "protected-example",
					status: "guarded",
					value: "sign-in required",
					detail: "missing session cookie",
				}),
			]),
		);
	});
});
