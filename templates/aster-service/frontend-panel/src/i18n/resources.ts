import enErrors from "@/i18n/locales/en-US/errors.json";
import enOperations from "@/i18n/locales/en-US/operations.json";
import enOverview from "@/i18n/locales/en-US/overview.json";
import enSettings from "@/i18n/locales/en-US/settings.json";
import enShell from "@/i18n/locales/en-US/shell.json";
import zhErrors from "@/i18n/locales/zh-CN/errors.json";
import zhOperations from "@/i18n/locales/zh-CN/operations.json";
import zhOverview from "@/i18n/locales/zh-CN/overview.json";
import zhSettings from "@/i18n/locales/zh-CN/settings.json";
import zhShell from "@/i18n/locales/zh-CN/shell.json";

export const resources = {
	"en-US": {
		shell: enShell,
		overview: enOverview,
		operations: enOperations,
		settings: enSettings,
		errors: enErrors,
	},
	"zh-CN": {
		shell: zhShell,
		overview: zhOverview,
		operations: zhOperations,
		settings: zhSettings,
		errors: zhErrors,
	},
} as const;
