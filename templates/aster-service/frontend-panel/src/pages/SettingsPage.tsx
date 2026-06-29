import { EndpointCode } from "@/components/ui/EndpointCode";

const settingsItems = [
	{ label: "Database", value: "ASTER__DATABASE__URL" },
	{ label: "Server bind", value: "ASTER__SERVER__HOST" },
	{ label: "Logging file", value: "disabled by default" },
] as const;

export function SettingsPage() {
	return (
		<div className="mx-auto grid max-w-4xl gap-5">
			<section className="grid gap-5 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900">
				<div className="grid gap-2">
					<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
						Settings
					</p>
					<h2 className="font-bold text-3xl leading-tight md:text-4xl">
						Keep template defaults small and make product choices explicit.
					</h2>
				</div>
				<div className="grid gap-3">
					{settingsItems.map((item) => (
						<div
							className="grid items-center gap-2 rounded-lg bg-slate-50 p-4 sm:grid-cols-[minmax(0,1fr)_auto] dark:bg-slate-800/60"
							key={item.label}
						>
							<span className="font-bold">{item.label}</span>
							<EndpointCode>{item.value}</EndpointCode>
						</div>
					))}
				</div>
			</section>
		</div>
	);
}
