const timelineItems = [
	{
		title: "Runtime",
		body: "Register product components in the Rust runtime assembly.",
	},
	{
		title: "Persistence",
		body: "Add product schema with SeaORM migrations and generated entities.",
	},
	{
		title: "Background work",
		body: "Attach scheduled tasks after the business contract is clear.",
	},
] as const;

export function OperationsPage() {
	return (
		<div className="mx-auto grid max-w-4xl gap-5">
			<section className="grid gap-5 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-800 dark:bg-slate-900">
				<div className="grid gap-2">
					<p className="font-bold text-blue-700 text-xs uppercase tracking-[0.08em] dark:text-blue-300">
						Operations
					</p>
					<h2 className="font-bold text-3xl leading-tight md:text-4xl">
						Wire real jobs, queues, and audit views here.
					</h2>
				</div>
				<div className="grid gap-3">
					{timelineItems.map((item) => (
						<div
							className="grid gap-2 rounded-lg bg-slate-50 p-4 dark:bg-slate-800/60"
							key={item.title}
						>
							<strong>{item.title}</strong>
							<p className="text-slate-600 leading-6 dark:text-slate-300">
								{item.body}
							</p>
						</div>
					))}
				</div>
			</section>
		</div>
	);
}
