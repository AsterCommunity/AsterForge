import { useEffect, useState } from "react";
import { EndpointCard } from "@/components/panel/EndpointCard";
import { NativeSelectField, TextField } from "@/components/panel/FormControls";
import { JsonPanel } from "@/components/panel/JsonPanel";
import { PageShell, SectionTitle } from "@/components/panel/PageShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	Card,
	CardContent,
	CardDescription,
	CardHeader,
	CardTitle,
} from "@/components/ui/card";
import { Icon } from "@/components/ui/icon";
import {
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableHeader,
	TableRow,
} from "@/components/ui/table";
import { useAsyncTask } from "@/hooks/useAsyncTask";
import { apiCatalog } from "@/lib/apiCatalog";
import { dateTimeLocalToIso, integerOrUndefined } from "@/lib/form";
import {
	auditActionBadgeClass,
	formatAuditDetail,
	formatAuditEntityType,
	formatAuditSummary,
	formatAuditTarget,
} from "@/lib/presentation";
import { adminAuditService } from "@/services/adminService";
import type {
	AdminAuditLogSortBy,
	AuditEntityType,
	AuditLogPage,
	SortOrder,
} from "@/types/api";

export default function AdminAuditPage() {
	const [limit, setLimit] = useState("50");
	const [offset, setOffset] = useState("0");
	const [userId, setUserId] = useState("");
	const [action, setAction] = useState("");
	const [entityType, setEntityType] = useState("");
	const [entityId, setEntityId] = useState("");
	const [after, setAfter] = useState("");
	const [before, setBefore] = useState("");
	const [sortBy, setSortBy] = useState<AdminAuditLogSortBy>("created_at");
	const [sortOrder, setSortOrder] = useState<SortOrder>("desc");
	const listTask = useAsyncTask<AuditLogPage>();
	const auditOps = apiCatalog.filter((item) => item.route === "/admin/audit");
	const auditItems = listTask.result?.items ?? [];

	function params() {
		return {
			limit: integerOrUndefined(limit),
			offset: integerOrUndefined(offset),
			user_id: integerOrUndefined(userId),
			action: action.trim() || undefined,
			entity_type: entityType ? (entityType as AuditEntityType) : undefined,
			entity_id: integerOrUndefined(entityId),
			after: dateTimeLocalToIso(after),
			before: dateTimeLocalToIso(before),
			sort_by: sortBy,
			sort_order: sortOrder,
		};
	}

	useEffect(() => {
		void listTask.run(() =>
			adminAuditService.list({
				limit: 50,
				offset: 0,
				sort_by: "created_at",
				sort_order: "desc",
			}),
		);
	}, [listTask.run]);

	return (
		<PageShell
			title="Admin Audit"
			description="Audit log query with filters, pagination, and sort controls."
		>
			<div className="grid gap-4 xl:grid-cols-[minmax(0,520px)_minmax(0,1fr)]">
				<Card>
					<CardHeader className="border-b border-border/60 pb-4">
						<CardTitle className="flex items-center gap-2">
							<Icon name="ClipboardText" className="size-4" />
							Audit query
						</CardTitle>
						<CardDescription>
							Action accepts any generated audit action wire value.
						</CardDescription>
					</CardHeader>
					<CardContent className="grid gap-3">
						<div className="grid gap-3 sm:grid-cols-2">
							<TextField label="Limit" value={limit} onChange={setLimit} />
							<TextField label="Offset" value={offset} onChange={setOffset} />
							<TextField label="User ID" value={userId} onChange={setUserId} />
							<TextField
								label="Entity ID"
								value={entityId}
								onChange={setEntityId}
							/>
							<TextField label="Action" value={action} onChange={setAction} />
							<NativeSelectField
								label="Entity type"
								value={entityType}
								onChange={setEntityType}
								options={[
									{ label: "Any", value: "" },
									{ label: "system", value: "system" },
									{ label: "system_config", value: "system_config" },
									{ label: "user", value: "user" },
									{ label: "auth_session", value: "auth_session" },
									{
										label: "external_auth_provider",
										value: "external_auth_provider",
									},
									{
										label: "external_auth_identity",
										value: "external_auth_identity",
									},
									{ label: "api_token", value: "api_token" },
									{ label: "task", value: "task" },
								]}
							/>
							<TextField
								label="After"
								type="datetime-local"
								value={after}
								onChange={setAfter}
							/>
							<TextField
								label="Before"
								type="datetime-local"
								value={before}
								onChange={setBefore}
							/>
							<NativeSelectField
								label="Sort by"
								value={sortBy}
								onChange={(value) => setSortBy(value as AdminAuditLogSortBy)}
								options={[
									{ label: "created_at", value: "created_at" },
									{ label: "id", value: "id" },
									{ label: "user_id", value: "user_id" },
									{ label: "action", value: "action" },
									{ label: "entity_type", value: "entity_type" },
									{ label: "entity_name", value: "entity_name" },
									{ label: "ip_address", value: "ip_address" },
								]}
							/>
							<NativeSelectField
								label="Sort order"
								value={sortOrder}
								onChange={(value) => setSortOrder(value as SortOrder)}
								options={[
									{ label: "desc", value: "desc" },
									{ label: "asc", value: "asc" },
								]}
							/>
						</div>
						<Button
							type="button"
							variant="outline"
							onClick={() =>
								void listTask.run(() => adminAuditService.list(params()))
							}
						>
							<Icon name="MagnifyingGlass" className="size-4" />
							Query
						</Button>
					</CardContent>
				</Card>
				<JsonPanel
					title="Audit query result"
					value={listTask.result}
					error={listTask.error}
					loading={listTask.loading}
				/>
			</div>

			<Card>
				<CardHeader className="border-b border-border/60 pb-4">
					<CardTitle>Audit logs</CardTitle>
				</CardHeader>
				<CardContent>
					{listTask.error ? (
						<JsonPanel title="List error" value={null} error={listTask.error} />
					) : (
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>ID</TableHead>
									<TableHead>Action</TableHead>
									<TableHead>Entity</TableHead>
									<TableHead>Detail</TableHead>
									<TableHead>User</TableHead>
									<TableHead>IP</TableHead>
									<TableHead>Created</TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{auditItems.map((item) => {
									const detail = formatAuditDetail(item);
									return (
										<TableRow key={item.id}>
											<TableCell>{item.id}</TableCell>
											<TableCell className="min-w-56 whitespace-normal">
												<div className="grid gap-1">
													<Badge
														variant="outline"
														className={auditActionBadgeClass(item.action)}
													>
														{formatAuditSummary(item)}
													</Badge>
													<span className="font-mono text-xs text-muted-foreground">
														{item.action}
													</span>
												</div>
											</TableCell>
											<TableCell className="min-w-56 whitespace-normal">
												<div className="grid gap-1">
													<span>{formatAuditTarget(item)}</span>
													<span className="font-mono text-xs text-muted-foreground">
														{formatAuditEntityType(item.entity_type)}
													</span>
												</div>
											</TableCell>
											<TableCell className="max-w-96 whitespace-normal text-muted-foreground">
												{detail ?? "-"}
											</TableCell>
											<TableCell>
												{item.user?.username ?? item.user_id}
											</TableCell>
											<TableCell>{item.ip_address ?? "-"}</TableCell>
											<TableCell className="font-mono text-xs">
												{item.created_at}
											</TableCell>
										</TableRow>
									);
								})}
							</TableBody>
						</Table>
					)}
				</CardContent>
			</Card>

			<div className="space-y-3">
				<SectionTitle
					title="Audit API coverage"
					description="The registered audit endpoint is covered by this query surface."
				/>
				<div className="grid gap-3 lg:grid-cols-2">
					{auditOps.map((item) => (
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
