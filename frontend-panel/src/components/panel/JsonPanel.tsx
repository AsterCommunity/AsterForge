import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Icon } from "@/components/ui/icon";
import { cn } from "@/lib/utils";

function stringify(value: unknown) {
	if (value === undefined) return "undefined";
	if (value === null) return "null";
	if (typeof value === "string") return value;
	return JSON.stringify(value, null, 2);
}

export function JsonPanel({
	title = "Result",
	value,
	error,
	loading,
	className,
}: {
	title?: string;
	value: unknown;
	error?: string | null;
	loading?: boolean;
	className?: string;
}) {
	const body = error
		? error
		: value === null || value === undefined
			? "No response yet"
			: stringify(value);

	return (
		<Card size="sm" className={cn("min-h-0", className)}>
			<CardHeader className="border-b border-border/60 pb-3">
				<CardTitle className="flex items-center gap-2 text-sm">
					{loading ? (
						<Icon name="Spinner" className="size-4 animate-spin" />
					) : (
						<Icon
							name={error ? "Warning" : "BracketsCurly"}
							className={cn("size-4", error ? "text-destructive" : "")}
						/>
					)}
					{title}
				</CardTitle>
			</CardHeader>
			<CardContent>
				<pre
					className={cn(
						"max-h-96 overflow-auto rounded-lg border border-border/70 bg-muted/45 p-3 font-mono text-xs leading-5 text-foreground",
						error ? "text-destructive" : "",
					)}
				>
					{body}
				</pre>
			</CardContent>
		</Card>
	);
}
