import { NavLink, Outlet } from "react-router-dom";
import { serviceName } from "@/config/app";

const navItems = [
	{ to: "/", label: "Overview", end: true },
	{ to: "/operations", label: "Operations", end: false },
	{ to: "/settings", label: "Settings", end: false },
] as const;

const navLinkBase =
	"rounded-lg border border-transparent px-3 py-2 text-sm font-semibold text-slate-500 transition hover:border-slate-200 hover:bg-white hover:text-slate-950 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500/40 dark:text-slate-400 dark:hover:border-slate-700 dark:hover:bg-slate-900 dark:hover:text-slate-50";
const navLinkActive =
	"border-slate-200 bg-white text-slate-950 shadow-sm dark:border-slate-700 dark:bg-slate-900 dark:text-slate-50";

export function AppShell() {
	return (
		<div className="grid min-h-svh bg-slate-50 text-slate-950 md:grid-cols-[280px_minmax(0,1fr)] dark:bg-slate-950 dark:text-slate-50">
			<aside
				className="flex flex-col gap-7 border-slate-200 border-b bg-slate-100/70 p-5 md:border-r md:border-b-0 md:p-7 dark:border-slate-800 dark:bg-slate-900/70"
				aria-label="Service navigation"
			>
				<div className="flex min-w-0 items-center gap-3.5">
					<img
						src="/favicon.svg"
						alt=""
						className="size-11 rounded-lg shadow-lg shadow-slate-900/15"
					/>
					<div className="min-w-0">
						<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
							Aster service
						</p>
						<h1 className="mt-0.5 font-bold text-lg leading-tight [overflow-wrap:anywhere]">
							{serviceName}
						</h1>
					</div>
				</div>

				<nav className="grid grid-cols-3 gap-1.5 md:grid-cols-1">
					{navItems.map((item) => (
						<NavLink
							key={item.to}
							to={item.to}
							end={item.end}
							className={({ isActive }) =>
								isActive ? `${navLinkBase} ${navLinkActive}` : navLinkBase
							}
						>
							{item.label}
						</NavLink>
					))}
				</nav>

				<div className="mt-auto hidden items-center gap-2.5 border-slate-200 border-t pt-5 text-slate-500 text-sm leading-relaxed md:flex dark:border-slate-800 dark:text-slate-400">
					<span className="size-2 rounded-full bg-teal-600 shadow-[0_0_0_4px_rgb(13_148_136_/_0.16)] dark:bg-teal-300" />
					<span>Template shell ready for product routes</span>
				</div>
			</aside>

			<main className="px-5 py-6 md:px-9 md:py-9">
				<Outlet />
			</main>
		</div>
	);
}
