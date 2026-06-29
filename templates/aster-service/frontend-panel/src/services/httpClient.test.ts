import { afterEach, describe, expect, it, vi } from "vitest";
import { requestJson } from "./httpClient";

describe("requestJson", () => {
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it("returns parsed JSON for successful responses", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(JSON.stringify({ status: "ok" }))),
		);

		await expect(requestJson<{ status: string }>("/health")).resolves.toEqual({
			status: "ok",
		});
	});

	it("throws HttpError with parsed payload for failed responses", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(
				async () =>
					new Response(JSON.stringify({ message: "not ready" }), {
						status: 503,
					}),
			),
		);

		await expect(requestJson("/health/ready")).rejects.toMatchObject({
			status: 503,
			payload: { message: "not ready" },
		});
	});
});
