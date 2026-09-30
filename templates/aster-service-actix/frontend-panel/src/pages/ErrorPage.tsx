import { useTranslation } from "react-i18next";
import { isRouteErrorResponse, Link, useRouteError } from "react-router-dom";
import { appPaths } from "@/routes/routePaths";

type ErrorContent = {
	code: string;
	title: string;
	description: string;
	detail: string | null;
};

type Translate = ReturnType<typeof useTranslation<"errors">>["t"];

function readRouteError(error: unknown, t: Translate): ErrorContent {
	if (isRouteErrorResponse(error)) {
		const isNotFound = error.status === 404;
		return {
			code: String(error.status || "ERR"),
			title: isNotFound ? t("notFoundTitle") : t("routeFailedTitle"),
			description: isNotFound
				? t("notFoundDescription")
				: t("routeFailedDescription"),
			detail: `${error.status} ${error.statusText}`.trim(),
		};
	}

	if (error instanceof Error) {
		return {
			code: "ERR",
			title: t("routeFailedTitle"),
			description: t("routeFailedDescription"),
			detail: error.message,
		};
	}

	return {
		code: "404",
		title: t("notFoundTitle"),
		description: t("notFoundDescription"),
		detail: null,
	};
}

export function ErrorPage() {
	const { t } = useTranslation("errors");
	const content = readRouteError(useRouteError(), t);

	return (
		<div className="grid min-h-[calc(100svh-4rem)] place-items-center px-5 py-10">
			<section className="w-full max-w-2xl rounded-lg border border-slate-200 bg-white p-6 shadow-xl shadow-slate-900/5 dark:border-slate-800 dark:bg-slate-900 dark:shadow-black/20">
				<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
					{content.code}
				</p>
				<h1 className="mt-3 font-bold text-3xl leading-tight md:text-4xl">
					{content.title}
				</h1>
				<p className="mt-4 text-slate-600 leading-7 dark:text-slate-300">
					{content.description}
				</p>
				{content.detail ? (
					<code className="mt-5 block overflow-x-auto rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 font-mono text-slate-700 text-sm dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200">
						{content.detail}
					</code>
				) : null}
				<Link
					to={appPaths.overview}
					className="mt-6 inline-flex h-10 items-center rounded-lg bg-slate-950 px-4 font-semibold text-sm text-white transition hover:bg-slate-800 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40 dark:bg-slate-50 dark:text-slate-950 dark:hover:bg-slate-200"
				>
					{t("backHome")}
				</Link>
			</section>
		</div>
	);
}
