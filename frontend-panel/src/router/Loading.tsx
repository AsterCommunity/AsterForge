import { Icon } from "@/components/ui/icon";

export function Loading() {
	return (
		<div className="flex min-h-64 items-center justify-center text-sm text-muted-foreground">
			<Icon name="Spinner" className="mr-2 size-4 animate-spin" />
			Loading
		</div>
	);
}
