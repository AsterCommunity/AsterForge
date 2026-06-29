import { create } from "zustand";
import {
	defaultLanguage,
	detectLanguage,
	normalizeLanguage,
	type SupportedLanguage,
} from "@/i18n";

const languageStorageKey = "aster-service.language";

function readInitialLanguage() {
	if (typeof window === "undefined") {
		return defaultLanguage;
	}

	const stored = normalizeLanguage(
		window.localStorage.getItem(languageStorageKey),
	);
	if (stored) {
		return stored;
	}

	return detectLanguage();
}

type UiPreferencesState = {
	language: SupportedLanguage;
	setLanguage: (language: SupportedLanguage) => void;
};

export const useUiPreferencesStore = create<UiPreferencesState>((set) => ({
	language: readInitialLanguage(),
	setLanguage(language) {
		if (typeof window !== "undefined") {
			window.localStorage.setItem(languageStorageKey, language);
		}
		set({ language });
	},
}));
