import { useTranslation } from "react-i18next";
import { EndpointCode } from "@/components/common/EndpointCode";
import { type SupportedLanguage, supportedLanguages } from "@/i18n";
import { useUiPreferencesStore } from "@/stores/uiPreferencesStore";

const settingsItems = [
	{ labelKey: "items.database", value: "ASTER__DATABASE__URL" },
	{ labelKey: "items.serverBind", value: "ASTER__SERVER__HOST" },
	{ labelKey: "items.loggingFile", valueKey: "items.loggingFileValue" },
] as const;

export function SettingsPage() {
	const { t } = useTranslation("settings");
	const language = useUiPreferencesStore((state) => state.language);
	const setLanguage = useUiPreferencesStore((state) => state.setLanguage);

	return (
		<div className="mx-auto grid max-w-4xl gap-5">
			<section className="grid gap-5 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900">
				<div className="grid gap-2">
					<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
						{t("eyebrow")}
					</p>
					<h2 className="font-bold text-3xl leading-tight md:text-4xl">
						{t("title")}
					</h2>
				</div>
				<label className="grid gap-2 rounded-lg bg-slate-50 p-4 dark:bg-slate-800/60">
					<span className="font-bold">{t("language.label")}</span>
					<select
						className="h-10 w-fit rounded-lg border border-slate-200 bg-white px-3 font-semibold text-sm dark:border-slate-700 dark:bg-slate-900"
						value={language}
						onChange={(event) =>
							setLanguage(event.target.value as SupportedLanguage)
						}
					>
						{supportedLanguages.map((item) => (
							<option key={item} value={item}>
								{t(`language.${item}`)}
							</option>
						))}
					</select>
				</label>
				<div className="grid gap-3">
					{settingsItems.map((item) => (
						<div
							className="grid items-center gap-2 rounded-lg bg-slate-50 p-4 sm:grid-cols-[minmax(0,1fr)_auto] dark:bg-slate-800/60"
							key={item.labelKey}
						>
							<span className="font-bold">{t(item.labelKey)}</span>
							<EndpointCode>
								{"value" in item ? item.value : t(item.valueKey)}
							</EndpointCode>
						</div>
					))}
				</div>
			</section>
		</div>
	);
}
