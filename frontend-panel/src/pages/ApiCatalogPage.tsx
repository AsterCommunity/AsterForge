import { useMemo, useState } from "react";
import { AccessBadge, PageShell } from "@/components/panel/PageShell";
import { Badge } from "@/components/ui/badge";
import {
	Card,
	CardContent,
	CardDescription,
	CardHeader,
	CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableHeader,
	TableRow,
} from "@/components/ui/table";
import { apiCatalog, getApiCatalogStats } from "@/lib/apiCatalog";

export default function ApiCatalogPage() {
	const [query, setQuery] = useState("");
	const stats = getApiCatalogStats();
	const filtered = useMemo(() => {
		const needle = query.trim().toLowerCase();
		if (!needle) return apiCatalog;
		return apiCatalog.filter((item) =>
			[
				item.operationId,
				item.method,
				item.path,
				item.group,
				item.route,
				item.access,
				item.description,
			]
				.join(" ")
				.toLowerCase()
				.includes(needle),
		);
	}, [query]);

	return (
		<PageShell
			title="API Catalog"
			description="Generated backend operations mapped to frontend routes and callable surfaces."
		>
			<div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
				<Card size="sm">
					<CardHeader>
						<CardTitle>Total</CardTitle>
						<CardDescription>OpenAPI operations</CardDescription>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.total}
					</CardContent>
				</Card>
				<Card size="sm">
					<CardHeader>
						<CardTitle>Groups</CardTitle>
						<CardDescription>Panel sections</CardDescription>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.groups}
					</CardContent>
				</Card>
				<Card size="sm">
					<CardHeader>
						<CardTitle>Admin</CardTitle>
						<CardDescription>Admin-only operations</CardDescription>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.admin}
					</CardContent>
				</Card>
				<Card size="sm">
					<CardHeader>
						<CardTitle>Public</CardTitle>
						<CardDescription>Unauthenticated operations</CardDescription>
					</CardHeader>
					<CardContent className="text-3xl font-semibold">
						{stats.public}
					</CardContent>
				</Card>
			</div>

			<Card>
				<CardHeader className="border-b border-border/60 pb-4">
					<CardTitle>Registered operation coverage</CardTitle>
					<CardDescription>
						The catalog is tested against generated/openapi.json.
					</CardDescription>
				</CardHeader>
				<CardContent className="space-y-3">
					<Input
						value={query}
						onChange={(event) => setQuery(event.currentTarget.value)}
						placeholder="Filter by operation, path, group, or route"
						aria-label="Filter API catalog"
					/>
					<Table>
						<TableHeader>
							<TableRow>
								<TableHead>Operation</TableHead>
								<TableHead>Method</TableHead>
								<TableHead>Path</TableHead>
								<TableHead>Access</TableHead>
								<TableHead>Coverage</TableHead>
								<TableHead>Route</TableHead>
							</TableRow>
						</TableHeader>
						<TableBody>
							{filtered.map((item) => (
								<TableRow key={item.operationId}>
									<TableCell className="font-mono text-xs">
										{item.operationId}
									</TableCell>
									<TableCell>
										<Badge
											variant={item.method === "GET" ? "outline" : "secondary"}
											className="font-mono"
										>
											{item.method}
										</Badge>
									</TableCell>
									<TableCell className="font-mono text-xs">
										{item.path}
									</TableCell>
									<TableCell>
										<AccessBadge access={item.access} />
									</TableCell>
									<TableCell>
										<Badge
											variant={item.destructive ? "destructive" : "outline"}
										>
											{item.coverage}
										</Badge>
									</TableCell>
									<TableCell className="font-mono text-xs">
										{item.route}
									</TableCell>
								</TableRow>
							))}
						</TableBody>
					</Table>
				</CardContent>
			</Card>
		</PageShell>
	);
}
