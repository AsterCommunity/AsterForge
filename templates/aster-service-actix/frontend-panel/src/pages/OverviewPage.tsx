import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { EndpointCode } from "@/components/common/EndpointCode";
import { backendPaths } from "@/routes/routePaths";
import { getHealth, getReadiness } from "@/services/healthService";

const serviceChecks = [
	{
		labelKey: "checks.http.label",
		valueKey: "checks.http.value",
		detailKey: "checks.http.detail",
	},
	{
		labelKey: "checks.readiness.label",
		value: backendPaths.readiness,
		detailKey: "checks.readiness.detail",
	},
	{
		labelKey: "checks.openapi.label",
		valueKey: "checks.openapi.value",
		detailKey: "checks.openapi.detail",
	},
] as const;

const apiSurfaces = [
	{ method: "GET", path: backendPaths.health, noteKey: "backend.notes.health" },
	{
		method: "GET",
		path: backendPaths.readiness,
		noteKey: "backend.notes.readiness",
	},
	{
		method: "GET",
		path: backendPaths.metrics,
		noteKey: "backend.notes.metrics",
	},
	{
		method: "GET",
		path: `${backendPaths.api}/*`,
		noteKey: "backend.notes.api",
	},
	{
		method: "GET",
		path: backendPaths.openapi,
		noteKey: "backend.notes.openapi",
	},
] as const;

type ProbeState = {
	health: string | null;
	readiness: string | null;
	error: string | null;
	loading: boolean;
};

export function OverviewPage() {
	const { t } = useTranslation("overview");
	const [probeState, setProbeState] = useState<ProbeState>({
		health: null,
		readiness: null,
		error: null,
		loading: false,
	});

	const refreshProbes = useCallback(async () => {
		setProbeState((current) => ({ ...current, loading: true, error: null }));
		try {
			const [health, readiness] = await Promise.all([
				getHealth(),
				getReadiness(),
			]);
			setProbeState({
				health: health.status,
				readiness: readiness.status,
				error: null,
				loading: false,
			});
		} catch (error) {
			setProbeState((current) => ({
				...current,
				error: error instanceof Error ? error.message : t("live.failed"),
				loading: false,
			}));
		}
	}, [t]);

	useEffect(() => {
		void refreshProbes();
	}, [refreshProbes]);

	return (
		<div className="mx-auto grid max-w-6xl gap-5">
			<section className="grid gap-7 rounded-lg border border-slate-200 bg-white p-6 shadow-xl shadow-slate-900/5 md:grid-cols-[minmax(0,1fr)_180px] md:items-end md:p-10 dark:border-slate-800 dark:bg-slate-900 dark:shadow-black/20">
				<div>
					<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
						{t("hero.eyebrow")}
					</p>
					<h2 className="mt-3 max-w-4xl font-bold text-3xl leading-tight md:text-5xl">
						{t("hero.title")}
					</h2>
					<p className="mt-5 max-w-3xl text-slate-600 leading-7 dark:text-slate-300">
						{t("hero.description")}
					</p>
				</div>
				<div className="grid justify-items-start gap-3 md:justify-items-center">
					<div className="grid size-28 place-items-center rounded-full bg-[conic-gradient(#0f766e_0_72%,#dbe4ef_72%)] dark:bg-[conic-gradient(#2dd4bf_0_72%,#334155_72%)]">
						<span className="grid size-20 place-items-center rounded-full bg-white font-bold text-4xl dark:bg-slate-900">
							{t("hero.routeCount")}
						</span>
					</div>
					<p className="font-semibold text-slate-500 text-sm dark:text-slate-400">
						{t("hero.routeCountLabel")}
					</p>
				</div>
			</section>

			<section
				className="grid gap-4 md:grid-cols-3"
				aria-label={t("checksLabel")}
			>
				{serviceChecks.map((check) => (
					<article
						className="grid min-h-36 gap-3 rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-800 dark:bg-slate-900"
						key={check.labelKey}
					>
						<div className="flex items-start justify-between gap-3">
							<span className="font-semibold text-slate-500 text-sm dark:text-slate-400">
								{t(check.labelKey)}
							</span>
							<strong className="font-bold text-sm text-teal-700 dark:text-teal-300">
								{"value" in check ? check.value : t(check.valueKey)}
							</strong>
						</div>
						<p className="text-slate-600 leading-6 dark:text-slate-300">
							{t(check.detailKey)}
						</p>
					</article>
				))}
			</section>

			<section className="grid gap-4 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900">
				<div className="flex flex-wrap items-center justify-between gap-3">
					<h3 className="font-bold text-xl leading-snug">{t("live.label")}</h3>
					<button
						type="button"
						className="h-9 rounded-lg border border-slate-200 bg-slate-50 px-3 font-semibold text-sm transition hover:bg-white disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:bg-slate-800 dark:hover:bg-slate-900"
						disabled={probeState.loading}
						onClick={() => void refreshProbes()}
					>
						{t("live.refresh")}
					</button>
				</div>
				<div className="grid gap-3 sm:grid-cols-2">
					<div className="rounded-lg bg-slate-50 p-4 dark:bg-slate-800/60">
						<span className="text-slate-500 text-sm dark:text-slate-400">
							{t("live.health")}
						</span>
						<p className="mt-1 font-bold text-lg">
							{probeState.health ?? t("live.unknown")}
						</p>
					</div>
					<div className="rounded-lg bg-slate-50 p-4 dark:bg-slate-800/60">
						<span className="text-slate-500 text-sm dark:text-slate-400">
							{t("live.readiness")}
						</span>
						<p className="mt-1 font-bold text-lg">
							{probeState.readiness ?? t("live.unknown")}
						</p>
					</div>
				</div>
				{probeState.error ? (
					<p className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-red-700 text-sm dark:border-red-400/30 dark:bg-red-400/10 dark:text-red-200">
						{probeState.error}
					</p>
				) : null}
			</section>

			<section className="grid gap-5 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900">
				<div className="grid gap-2">
					<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
						{t("backend.eyebrow")}
					</p>
					<h3 className="font-bold text-2xl leading-snug">
						{t("backend.title")}
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
								{t(endpoint.noteKey)}
							</p>
						</div>
					))}
				</div>
			</section>
		</div>
	);
}
