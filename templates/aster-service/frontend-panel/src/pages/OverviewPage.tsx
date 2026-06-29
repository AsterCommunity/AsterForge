import { EndpointCode } from "@/components/ui/EndpointCode";

const serviceChecks = [
	{
		label: "HTTP runtime",
		value: "Serving",
		detail: "Actix routes are mounted and ready for product APIs.",
	},
	{
		label: "Readiness",
		value: "/health/ready",
		detail: "Use readiness for deployment probes and rollout gates.",
	},
	{
		label: "OpenAPI",
		value: "Generated",
		detail: "Run the OpenAPI test to refresh frontend-panel/generated.",
	},
] as const;

const apiSurfaces = [
	{ method: "GET", path: "/health", note: "Liveness probe" },
	{ method: "GET", path: "/health/ready", note: "Readiness probe" },
	{
		method: "GET",
		path: "/health/metrics",
		note: "Prometheus metrics with metrics feature",
	},
	{ method: "GET", path: "/api/v1/*", note: "Product API routes" },
	{ method: "GET", path: "/api-docs/openapi.json", note: "Debug OpenAPI spec" },
] as const;

export function OverviewPage() {
	return (
		<div className="mx-auto grid max-w-6xl gap-5">
			<section className="grid gap-7 rounded-lg border border-slate-200 bg-white p-6 shadow-xl shadow-slate-900/5 md:grid-cols-[minmax(0,1fr)_180px] md:items-end md:p-10 dark:border-slate-800 dark:bg-slate-900 dark:shadow-black/20">
				<div>
					<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
						Runtime foundation
					</p>
					<h2 className="mt-3 max-w-4xl font-bold text-3xl leading-tight md:text-5xl">
						Start from the service core, then replace this panel with product
						work.
					</h2>
					<p className="mt-5 max-w-3xl text-slate-600 leading-7 dark:text-slate-300">
						The generated backend already owns configuration loading, health
						probes, metrics, migrations, embedded frontend assets, and the
						runtime assembly.
					</p>
				</div>
				<div className="grid justify-items-start gap-3 md:justify-items-center">
					<div className="grid size-28 place-items-center rounded-full bg-[conic-gradient(#0f766e_0_72%,#dbe4ef_72%)] dark:bg-[conic-gradient(#2dd4bf_0_72%,#334155_72%)]">
						<span className="grid size-20 place-items-center rounded-full bg-white font-bold text-4xl dark:bg-slate-900">
							3
						</span>
					</div>
					<p className="font-semibold text-slate-500 text-sm dark:text-slate-400">
						foundation routes wired
					</p>
				</div>
			</section>

			<section
				className="grid gap-4 md:grid-cols-3"
				aria-label="Service checks"
			>
				{serviceChecks.map((check) => (
					<article
						className="grid min-h-36 gap-3 rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-800 dark:bg-slate-900"
						key={check.label}
					>
						<div className="flex items-start justify-between gap-3">
							<span className="font-semibold text-slate-500 text-sm dark:text-slate-400">
								{check.label}
							</span>
							<strong className="font-bold text-sm text-teal-700 dark:text-teal-300">
								{check.value}
							</strong>
						</div>
						<p className="text-slate-600 leading-6 dark:text-slate-300">
							{check.detail}
						</p>
					</article>
				))}
			</section>

			<section className="grid gap-5 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900">
				<div className="grid gap-2">
					<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
						Backend surfaces
					</p>
					<h3 className="font-bold text-2xl leading-snug">
						Keep infrastructure endpoints boring and predictable.
					</h3>
				</div>
				<div className="grid gap-2.5">
					{apiSurfaces.map((endpoint) => (
						<div
							className="grid items-center gap-3 rounded-lg border border-slate-200/80 bg-slate-50 p-3 sm:grid-cols-[72px_minmax(170px,0.8fr)_minmax(0,1fr)] dark:border-slate-700/80 dark:bg-slate-800/60"
							key={endpoint.path}
						>
							<EndpointCode>{endpoint.method}</EndpointCode>
							<span className="font-bold break-all">{endpoint.path}</span>
							<p className="text-slate-600 leading-6 dark:text-slate-300">
								{endpoint.note}
							</p>
						</div>
					))}
				</div>
			</section>
		</div>
	);
}
