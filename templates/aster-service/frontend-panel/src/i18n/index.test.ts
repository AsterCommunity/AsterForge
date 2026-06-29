import { describe, expect, it } from "vitest";
import { defaultLanguage, normalizeLanguage } from "./index";

describe("i18n language normalization", () => {
	it("normalizes supported browser language values", () => {
		expect(normalizeLanguage("zh-Hans-CN")).toBe("zh-CN");
		expect(normalizeLanguage("en-GB")).toBe("en-US");
		expect(normalizeLanguage("zh-CN")).toBe("zh-CN");
		expect(normalizeLanguage("en-US")).toBe("en-US");
	});

	it("rejects unsupported values", () => {
		expect(normalizeLanguage("fr-FR")).toBeNull();
		expect(normalizeLanguage(null)).toBeNull();
		expect(defaultLanguage).toBe("en-US");
	});
});
