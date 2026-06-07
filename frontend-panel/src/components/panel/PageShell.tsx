import type { ReactNode } from "react";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

export function PageShell({
	title,
	description,
	children,
	actions,
}: {
	title: string;
	description: string;
	children: ReactNode;
	actions?: ReactNode;
}) {
	return (
		<section className="mx-auto flex w-full max-w-7xl flex-col gap-5 px-4 py-5 sm:px-6 lg:px-8">
			<header className="flex flex-col gap-3 border-b border-border/70 pb-4 md:flex-row md:items-end md:justify-between">
				<div className="min-w-0">
					<h1 className="text-2xl font-semibold tracking-normal text-foreground">
						{title}
					</h1>
					<p className="mt-1 max-w-3xl text-sm leading-6 text-muted-foreground">
						{description}
					</p>
				</div>
				{actions ? <div className="flex shrink-0 gap-2">{actions}</div> : null}
			</header>
			{children}
		</section>
	);
}

export function SectionTitle({
	title,
	description,
	className,
}: {
	title: string;
	description?: string;
	className?: string;
}) {
	return (
		<div className={cn("min-w-0", className)}>
			<h2 className="text-base font-medium text-foreground">{title}</h2>
			{description ? (
				<p className="mt-1 text-sm leading-5 text-muted-foreground">
					{description}
				</p>
			) : null}
		</div>
	);
}

export function AccessBadge({
	access,
}: {
	access: "public" | "auth" | "admin";
}) {
	const variant =
		access === "admin"
			? "destructive"
			: access === "auth"
				? "secondary"
				: "outline";
	return <Badge variant={variant}>{access}</Badge>;
}
