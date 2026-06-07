import { Link } from "react-router-dom";
import { EndpointCard, MethodBadge } from "@/components/panel/EndpointCard";
import { JsonPanel } from "@/components/panel/JsonPanel";
import { PageShell, SectionTitle } from "@/components/panel/PageShell";
import { Badge } from "@/components/ui/badge";
import { buttonVariants } from "@/components/ui/buttonVariants";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Icon } from "@/components/ui/icon";
import {
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableHeader,
	TableRow,
} from "@/components/ui/table";
import { useServiceDiagnostics } from "@/hooks/useServiceDiagnostics";
import { apiCatalog, getApiCatalogStats } from "@/lib/apiCatalog";
import { cn } from "@/lib/utils";

function statusBadge(status: string) {
	if (status === "ok") return <Badge>ok</Badge>;
	if (status === "guarded") return <Badge variant="secondary">guarded</Badge>;
	if (status === "error") return <Badge variant="destructive">error</Badge>;
	return <Badge variant="outline">{status}</Badge>;
}

export default function OverviewPage() {
	const diagnostics = useServiceDiagnostics();
	const stats = getApiCatalogStats();
	const overviewOps = apiCatalog.filter((item) => item.route === "/overview");

	return (
		<PageShell
			title="AsterForge"
			description="Service runtime, auth bootstrap, and generated API coverage in one operational panel."
			actions={
				<Link
					to="/api-catalog"
					className={cn(buttonVariants({ variant: "outline", size: "sm" }))}
				>
					<Icon name="Table" className="size-4" />
					API Catalog
				</Link>
			}
		>
			<div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
				<Card size="sm">
					<CardHeader>
						<CardTitle>Registered APIs</CardTitle>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.total}
					</CardContent>
				</Card>
				<Card size="sm">
					<CardHeader>
						<CardTitle>Admin APIs</CardTitle>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.admin}
					</CardContent>
				</Card>
				<Card size="sm">
					<CardHeader>
						<CardTitle>Auth APIs</CardTitle>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.auth}
					</CardContent>
				</Card>
				<Card size="sm">
					<CardHeader>
						<CardTitle>Public APIs</CardTitle>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.public}
					</CardContent>
				</Card>
			</div>

			<div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_360px]">
				<Card>
					<CardHeader className="border-b border-border/60 pb-4">
						<CardTitle className="flex items-center gap-2">
							<Icon name="Gauge" className="size-4" />
							Live service calls
						</CardTitle>
					</CardHeader>
					<CardContent>
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>Endpoint</TableHead>
									<TableHead>Status</TableHead>
									<TableHead>Value</TableHead>
									<TableHead>Detail</TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{diagnostics.endpoints.map((endpoint) => (
									<TableRow key={endpoint.id}>
										<TableCell>
											<div className="flex items-center gap-2">
												<MethodBadge method={endpoint.method} />
												<span className="font-mono text-xs">
													{endpoint.path}
												</span>
											</div>
										</TableCell>
										<TableCell>{statusBadge(endpoint.status)}</TableCell>
										<TableCell>{endpoint.value}</TableCell>
										<TableCell className="max-w-72 truncate text-muted-foreground">
											{endpoint.detail ?? endpoint.error ?? "-"}
										</TableCell>
									</TableRow>
								))}
							</TableBody>
						</Table>
					</CardContent>
				</Card>
				<JsonPanel
					title="Diagnostics snapshot"
					value={{
						loading: diagnostics.loading,
						updatedAt: diagnostics.updatedAt,
						error: diagnostics.error,
						endpoints: diagnostics.endpoints,
					}}
					loading={diagnostics.loading}
					error={diagnostics.error}
				/>
			</div>

			<div className="space-y-3">
				<SectionTitle
					title="Overview API coverage"
					description="These runtime and example endpoints are called from this page."
				/>
				<div className="grid gap-3 lg:grid-cols-2">
					{overviewOps.map((item) => (
						<EndpointCard
							key={item.operationId}
							title={item.operationId}
							method={item.method}
							path={item.path}
							description={item.description}
						/>
					))}
				</div>
			</div>
		</PageShell>
	);
}
