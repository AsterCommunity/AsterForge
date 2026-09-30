import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { resources } from "@/i18n/resources";

export const supportedLanguages = ["en-US", "zh-CN"] as const;
export type SupportedLanguage = (typeof supportedLanguages)[number];

export const defaultLanguage: SupportedLanguage = "en-US";

export function normalizeLanguage(value: string | null | undefined) {
	if (!value) return null;

	if (supportedLanguages.includes(value as SupportedLanguage)) {
		return value as SupportedLanguage;
	}

	const normalized = value.toLowerCase();
	if (normalized.startsWith("zh")) {
		return "zh-CN";
	}
	if (normalized.startsWith("en")) {
		return "en-US";
	}

	return null;
}

export function detectLanguage() {
	if (typeof navigator === "undefined") {
		return defaultLanguage;
	}

	for (const language of navigator.languages ?? [navigator.language]) {
		const supported = normalizeLanguage(language);
		if (supported) return supported;
	}

	return defaultLanguage;
}

void i18next.use(initReactI18next).init({
	resources,
	lng: defaultLanguage,
	fallbackLng: defaultLanguage,
	defaultNS: "shell",
	ns: ["shell", "overview", "operations", "settings", "errors"],
	interpolation: {
		escapeValue: false,
	},
});

export { i18next };
