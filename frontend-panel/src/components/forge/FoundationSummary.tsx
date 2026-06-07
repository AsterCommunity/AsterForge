import { Icon } from "@/components/ui/icon";

const cards = [
	{
		label: "Runtime",
		title: "Rust service foundation",
		body: "Actix Web, SeaORM migrations, static config, runtime config, cache, health checks, embedded frontend assets, and request response conventions are wired in.",
		icon: "Cpu",
		iconClassName: "bg-slate-950 text-white",
	},
	{
		label: "Identity",
		title: "Local auth plus SSO hooks",
		body: "First-admin setup, registration, HttpOnly auth cookies, refresh sessions, and provider-driven external auth start/callback routes are available as extension points.",
		icon: "Shield",
		iconClassName: "bg-primary text-primary-foreground",
	},
] as const;

export function FoundationSummary() {
	return (
		<section className="grid gap-4 lg:grid-cols-[1.1fr_0.9fr]">
			{cards.map((card) => (
				<article
					className="grid grid-cols-[3.25rem_minmax(0,1fr)] gap-4 rounded-lg border border-border bg-card/80 p-5 shadow-sm backdrop-blur"
					key={card.title}
				>
					<div
						className={`grid size-11 place-items-center rounded-md ${card.iconClassName}`}
					>
						<Icon name={card.icon} className="size-5" />
					</div>
					<div>
						<p className="mb-1 text-xs font-bold uppercase text-muted-foreground tracking-[0.08em]">
							{card.label}
						</p>
						<h2 className="mb-2 text-xl font-semibold text-foreground">
							{card.title}
						</h2>
						<p className="text-sm leading-6 text-muted-foreground">
							{card.body}
						</p>
					</div>
				</article>
			))}
		</section>
	);
}
