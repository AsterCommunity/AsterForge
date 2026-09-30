import { useTranslation } from "react-i18next";

const timelineItems = [
	{
		titleKey: "items.runtime.title",
		bodyKey: "items.runtime.body",
	},
	{
		titleKey: "items.persistence.title",
		bodyKey: "items.persistence.body",
	},
	{
		titleKey: "items.background.title",
		bodyKey: "items.background.body",
	},
] as const;

export function OperationsPage() {
	const { t } = useTranslation("operations");

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
				<div className="grid gap-3">
					{timelineItems.map((item) => (
						<div
							className="grid gap-2 rounded-lg bg-slate-50 p-4 dark:bg-slate-800/60"
							key={item.titleKey}
						>
							<strong>{t(item.titleKey)}</strong>
							<p className="text-slate-600 leading-6 dark:text-slate-300">
								{t(item.bodyKey)}
							</p>
						</div>
					))}
				</div>
			</section>
		</div>
	);
}
