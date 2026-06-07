import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { apiCatalog } from "./apiCatalog";

type OpenApiDocument = {
	paths: Record<
		string,
		Record<string, { operationId?: string } | undefined> | undefined
	>;
};

const httpMethods = new Set(["get", "post", "put", "patch", "delete"]);

function readGeneratedOperationIds() {
	const raw = readFileSync(
		resolve(process.cwd(), "generated/openapi.json"),
		"utf8",
	);
	const document = JSON.parse(raw) as OpenApiDocument;
	const operationIds: string[] = [];

	for (const pathItem of Object.values(document.paths)) {
		if (!pathItem) continue;
		for (const [method, operation] of Object.entries(pathItem)) {
			if (!httpMethods.has(method)) continue;
			if (operation?.operationId) {
				operationIds.push(operation.operationId);
			}
		}
	}

	return operationIds.sort();
}

describe("apiCatalog", () => {
	it("covers every generated backend operation exactly once", () => {
		const generatedOperationIds = readGeneratedOperationIds();
		const catalogOperationIds = apiCatalog
			.map((item) => item.operationId)
			.sort();

		expect(catalogOperationIds).toEqual(generatedOperationIds);
		expect(new Set(catalogOperationIds).size).toBe(catalogOperationIds.length);
		expect(catalogOperationIds).toHaveLength(37);
	});
});
