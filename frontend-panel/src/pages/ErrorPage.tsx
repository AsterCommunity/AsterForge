import { isRouteErrorResponse, useRouteError } from "react-router-dom";
import { PageShell } from "@/components/panel/PageShell";
import { Button } from "@/components/ui/button";
import { Icon } from "@/components/ui/icon";

function readRouteError(error: unknown) {
	if (isRouteErrorResponse(error)) {
		return `${error.status} ${error.statusText}`;
	}
	if (error instanceof Error) return error.message;
	return "Route failed";
}

export default function ErrorPage() {
	const error = useRouteError();

	return (
		<PageShell
			title="Route error"
			description="The current view failed before it could render."
			actions={
				<Button type="button" variant="outline" onClick={() => history.back()}>
					<Icon name="ArrowLeft" className="size-4" />
					Back
				</Button>
			}
		>
			<div className="rounded-lg border border-destructive/30 bg-destructive/10 p-4 text-sm text-destructive">
				{readRouteError(error)}
			</div>
		</PageShell>
	);
}
