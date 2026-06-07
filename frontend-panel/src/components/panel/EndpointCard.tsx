import type { ReactNode } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	Card,
	CardAction,
	CardContent,
	CardDescription,
	CardHeader,
	CardTitle,
} from "@/components/ui/card";
import { Icon } from "@/components/ui/icon";
import { cn } from "@/lib/utils";

export function MethodBadge({
	method,
	destructive,
}: {
	method: string;
	destructive?: boolean;
}) {
	const variant =
		method === "GET" ? "outline" : destructive ? "destructive" : "secondary";
	return (
		<Badge variant={variant} className="font-mono">
			{method}
		</Badge>
	);
}

export function EndpointCard({
	title,
	method,
	path,
	description,
	children,
	actionLabel,
	onAction,
	loading,
	destructive,
	className,
}: {
	title: string;
	method: string;
	path: string;
	description?: string;
	children?: ReactNode;
	actionLabel?: string;
	onAction?: () => void;
	loading?: boolean;
	destructive?: boolean;
	className?: string;
}) {
	return (
		<Card size="sm" className={cn("min-w-0", className)}>
			<CardHeader>
				<CardTitle className="flex min-w-0 items-center gap-2">
					<MethodBadge method={method} destructive={destructive} />
					<span className="truncate">{title}</span>
				</CardTitle>
				<CardDescription className="font-mono text-xs">{path}</CardDescription>
				{onAction ? (
					<CardAction>
						<Button
							type="button"
							size="sm"
							variant={destructive ? "destructive" : "outline"}
							onClick={onAction}
							disabled={loading}
						>
							<Icon
								name={loading ? "Spinner" : "Play"}
								className={cn("size-4", loading ? "animate-spin" : "")}
							/>
							{actionLabel ?? "Call"}
						</Button>
					</CardAction>
				) : null}
			</CardHeader>
			{description || children ? (
				<CardContent className="space-y-3">
					{description ? (
						<p className="text-sm leading-5 text-muted-foreground">
							{description}
						</p>
					) : null}
					{children}
				</CardContent>
			) : null}
		</Card>
	);
}
