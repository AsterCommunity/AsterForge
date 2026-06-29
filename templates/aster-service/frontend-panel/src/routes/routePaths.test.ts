import { describe, expect, it } from "vitest";
import { appPaths, backendPathDenylist, backendPaths } from "./routePaths";

describe("route paths", () => {
	it("keeps app shell paths centralized", () => {
		expect(appPaths).toEqual({
			overview: "/",
			operations: "/operations",
			settings: "/settings",
		});
	});

	it.each([
		backendPaths.api,
		backendPaths.health,
		backendPaths.readiness,
		backendPaths.metrics,
		backendPaths.openapi,
		backendPaths.swaggerUi,
	])("keeps backend path %s out of the SPA fallback", (path) => {
		expect(backendPathDenylist.some((pattern) => pattern.test(path))).toBe(
			true,
		);
	});
});
