import { Icon } from "@/components/ui/icon";
import type { ServiceDiagnosticResult } from "@/services/diagnosticsService";

type ServiceDiagnosticsPanelProps = {
	endpoints: ServiceDiagnosticResult[];
	loading: boolean;
	updatedAt: string | null;
	error: string | null;
	onRefresh: () => void;
};

const statusPresentation = {
	idle: {
		label: "idle",
		className: "border-muted bg-muted text-muted-foreground",
		icon: "Clock",
	},
	loading: {
		label: "checking",
		className: "border-sky-200 bg-sky-50 text-sky-700",
		icon: "Spinner",
	},
	ok: {
		label: "ok",
		className: "border-emerald-200 bg-emerald-50 text-emerald-700",
		icon: "Check",
	},
	guarded: {
		label: "guarded",
		className: "border-amber-200 bg-amber-50 text-amber-800",
		icon: "Lock",
	},
	error: {
		label: "error",
		className: "border-red-200 bg-red-50 text-red-700",
		icon: "Warning",
	},
} as const;

function formatUpdatedAt(value: string | null) {
	if (!value) return "not checked";
	return new Intl.DateTimeFormat(undefined, {
		hour: "2-digit",
		minute: "2-digit",
		second: "2-digit",
	}).format(new Date(value));
}

export function ServiceDiagnosticsPanel({
	endpoints,
	loading,
	updatedAt,
	error,
	onRefresh,
}: ServiceDiagnosticsPanelProps) {
	return (
		<section
			className="mt-4 overflow-hidden rounded-lg border border-border bg-card/80 shadow-sm backdrop-blur"
			aria-labelledby="diagnostics-heading"
		>
			<div className="flex flex-col gap-3 border-b border-border px-5 py-4 sm:flex-row sm:items-center sm:justify-between">
				<div>
					<p className="mb-1 text-xs font-bold uppercase text-muted-foreground tracking-[0.08em]">
						Registered API Probes
					</p>
					<h2
						id="diagnostics-heading"
						className="text-xl font-semibold text-foreground"
					>
						Live service calls
					</h2>
					<p className="mt-1 text-sm text-muted-foreground">
						last check: {formatUpdatedAt(updatedAt)}
					</p>
				</div>
				<button
					type="button"
					className="inline-flex min-h-10 w-fit items-center gap-2 rounded-md border border-border bg-background px-3 text-sm font-semibold text-foreground shadow-sm transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
					onClick={onRefresh}
					disabled={loading}
				>
					<Icon
						name="ArrowsClockwise"
						className={`size-4 ${loading ? "animate-spin" : ""}`}
					/>
					Refresh
				</button>
			</div>
			{error ? (
				<div className="border-b border-red-200 bg-red-50 px-5 py-3 text-sm text-red-700">
					{error}
				</div>
			) : null}
			<ul className="grid">
				{endpoints.map((endpoint) => {
					const status = statusPresentation[endpoint.status];
					return (
						<li
							className="grid gap-3 border-b border-border px-5 py-4 last:border-b-0 md:grid-cols-[12rem_minmax(0,1fr)_9rem] md:items-center"
							key={endpoint.id}
						>
							<div className="flex items-center gap-3">
								<div className="grid size-10 place-items-center rounded-md bg-slate-950 text-white">
									<Icon name={endpoint.icon} className="size-5" />
								</div>
								<div>
									<p className="text-sm font-semibold text-foreground">
										{endpoint.label}
									</p>
									<p className="text-xs text-muted-foreground">
										{endpoint.group}
									</p>
								</div>
							</div>
							<div className="min-w-0">
								<div className="flex flex-wrap items-center gap-2">
									<span className="inline-grid min-h-6 w-12 place-items-center rounded-md bg-slate-950 text-xs font-bold text-white">
										{endpoint.method}
									</span>
									<code className="font-mono text-sm text-foreground [overflow-wrap:anywhere]">
										{endpoint.path}
									</code>
								</div>
								<p className="mt-2 text-sm leading-5 text-muted-foreground">
									<span className="font-medium text-foreground">
										{endpoint.value}
									</span>
									{endpoint.detail ? ` · ${endpoint.detail}` : ""}
									{endpoint.error ? ` · ${endpoint.error}` : ""}
								</p>
							</div>
							<span
								className={`inline-flex min-h-8 w-fit items-center gap-2 rounded-md border px-2.5 text-xs font-bold uppercase tracking-[0.08em] md:justify-self-end ${status.className}`}
							>
								<Icon
									name={status.icon}
									className={`size-4 ${
										endpoint.status === "loading" ? "animate-spin" : ""
									}`}
								/>
								{status.label}
							</span>
						</li>
					);
				})}
			</ul>
		</section>
	);
}
