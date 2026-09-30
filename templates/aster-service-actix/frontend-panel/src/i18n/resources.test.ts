import { describe, expect, it } from "vitest";
import { resources } from "./resources";

function flattenKeys(value: unknown, prefix = ""): string[] {
	if (!value || typeof value !== "object") {
		return [prefix];
	}

	return Object.entries(value).flatMap(([key, child]) =>
		flattenKeys(child, prefix ? `${prefix}.${key}` : key),
	);
}

describe("i18n resources", () => {
	it("keeps zh-CN namespace keys aligned with en-US", () => {
		const english = resources["en-US"];
		const chinese = resources["zh-CN"];

		for (const namespace of Object.keys(english) as Array<
			keyof typeof english
		>) {
			expect(flattenKeys(chinese[namespace]).sort()).toEqual(
				flattenKeys(english[namespace]).sort(),
			);
		}
	});
});
