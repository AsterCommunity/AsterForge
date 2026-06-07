import { cn } from "@/lib/utils";

function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
	return (
		<div
			data-slot="skeleton"
			className={cn(
				"animate-pulse rounded-md bg-muted/75 dark:bg-muted/55",
				className,
			)}
			{...props}
		/>
	);
}

export { Skeleton };
