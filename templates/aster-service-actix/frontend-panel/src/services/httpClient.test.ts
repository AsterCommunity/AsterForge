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

	it("preserves known and unknown REST envelope codes and metadata", async () => {
		for (const code of ["endpoint_not_found", "future.product_code"]) {
			const payload = {
				code,
				msg: "product message",
				error: { retryable: false },
			};
			vi.stubGlobal(
				"fetch",
				vi.fn(
					async () => new Response(JSON.stringify(payload), { status: 404 }),
				),
			);
			await expect(requestJson("/api/v1/missing")).rejects.toMatchObject({
				status: 404,
				payload,
			});
		}
	});

	it("preserves successful absent, null, object and collection data", async () => {
		for (const payload of [
			{ code: "success", msg: "" },
			{ code: "success", msg: "", data: null },
			{ code: "success", msg: "", data: {} },
			{ code: "success", msg: "", data: [] },
		]) {
			vi.stubGlobal(
				"fetch",
				vi.fn(async () => new Response(JSON.stringify(payload))),
			);
			await expect(requestJson("/api/v1/demo")).resolves.toEqual(payload);
		}
	});

	it("reports non-JSON and empty HTTP failures through HttpError", async () => {
		for (const text of ["upstream unavailable", ""]) {
			vi.stubGlobal(
				"fetch",
				vi.fn(async () => new Response(text, { status: 503 })),
			);
			await expect(requestJson("/api/v1/demo")).rejects.toMatchObject({
				name: "HttpError",
				status: 503,
				payload: null,
			});
		}
	});

	it("rejects malformed JSON on successful HTTP responses", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response("not JSON")),
		);
		await expect(requestJson("/api/v1/demo")).rejects.toThrow(
			"Invalid JSON response",
		);
	});
});
