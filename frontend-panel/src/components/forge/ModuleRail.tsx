import { Icon } from "@/components/ui/icon";

const modules = [
	{ label: "Runtime", icon: "Gauge" },
	{ label: "Authentication", icon: "Key" },
	{ label: "API", icon: "BracketsCurly" },
] as const;

export function ModuleRail() {
	return (
		<aside
			className="flex min-h-16 items-center justify-center gap-3 bg-sidebar-foreground px-4 py-3 text-sidebar md:min-h-screen md:flex-col md:justify-start md:px-3 md:py-5"
			aria-label="AsterForge modules"
		>
			<div className="mr-auto grid size-11 place-items-center rounded-md bg-amber-300 font-bold text-slate-950 md:mr-0 md:mb-3">
				AF
			</div>
			{modules.map((module) => (
				<button
					className="grid size-11 place-items-center rounded-md border border-white/15 bg-white/5 text-white transition-colors hover:bg-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
					type="button"
					aria-label={module.label}
					key={module.label}
				>
					<Icon name={module.icon} className="size-5" />
				</button>
			))}
		</aside>
	);
}
