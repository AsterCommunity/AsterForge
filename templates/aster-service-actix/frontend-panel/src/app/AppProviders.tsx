import type { ReactNode } from "react";
import { useEffect } from "react";
import { i18next } from "@/i18n";
import { useUiPreferencesStore } from "@/stores/uiPreferencesStore";

export function AppProviders({ children }: { children: ReactNode }) {
	const language = useUiPreferencesStore((state) => state.language);

	useEffect(() => {
		if (i18next.language !== language) {
			void i18next.changeLanguage(language);
		}
		document.documentElement.lang = language;
		document.documentElement.dir = i18next.dir(language);
	}, [language]);

	return children;
}
