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
	formatTaskKind,
	formatTaskStatusDetail,
	formatTaskStatusLabel,
	formatTaskTitle,
	taskStatusBadgeVariant,
} from "@/lib/presentation";
import { adminTaskService } from "@/services/adminService";
import type {
	AdminTaskPage,
	AdminTaskSortBy,
	BackgroundTaskKind,
	BackgroundTaskStatus,
	SortOrder,
} from "@/types/api";

const taskStatuses = [
	"",
	"pending",
	"processing",
	"retry",
	"succeeded",
	"failed",
	"canceled",
] as const;

export default function AdminTasksPage() {
	const [limit, setLimit] = useState("50");
	const [offset, setOffset] = useState("0");
	const [kind, setKind] = useState("");
	const [status, setStatus] = useState("");
	const [sortBy, setSortBy] = useState<AdminTaskSortBy>("updated_at");
	const [sortOrder, setSortOrder] = useState<SortOrder>("desc");
	const [retryId, setRetryId] = useState("");
	const [finishedBefore, setFinishedBefore] = useState("");
	const task = useAsyncTask<unknown>();
	const listTask = useAsyncTask<AdminTaskPage>();
	const taskOps = apiCatalog.filter((item) => item.route === "/admin/tasks");
	const numericRetryId = Number(retryId);
	const hasRetryId = Number.isFinite(numericRetryId) && numericRetryId > 0;
	const cleanupDate = dateTimeLocalToIso(finishedBefore);

	function listParams() {
		return {
			limit: integerOrUndefined(limit),
			offset: integerOrUndefined(offset),
			kind: kind ? (kind as BackgroundTaskKind) : undefined,
			status: status ? (status as BackgroundTaskStatus) : undefined,
			sort_by: sortBy,
			sort_order: sortOrder,
		};
	}

	useEffect(() => {
		void listTask.run(() =>
			adminTaskService.list({
				limit: 50,
				offset: 0,
				sort_by: "updated_at",
				sort_order: "desc",
			}),
		);
	}, [listTask.run]);

	return (
		<PageShell
			title="Admin Tasks"
			description="Background task listing, cleanup, and retry controls."
		>
			<div className="grid gap-4 xl:grid-cols-[minmax(0,520px)_minmax(0,1fr)]">
				<Card>
					<CardHeader className="border-b border-border/60 pb-4">
						<CardTitle className="flex items-center gap-2">
							<Icon name="Clock" className="size-4" />
							Task filters and actions
						</CardTitle>
						<CardDescription>
							Cleanup requires an explicit finished-before timestamp.
						</CardDescription>
					</CardHeader>
					<CardContent className="grid gap-3">
						<div className="grid gap-3 sm:grid-cols-2">
							<TextField label="Limit" value={limit} onChange={setLimit} />
							<TextField label="Offset" value={offset} onChange={setOffset} />
							<NativeSelectField
								label="Kind"
								value={kind}
								onChange={setKind}
								options={[
									{ label: "Any", value: "" },
									{ label: "system_runtime", value: "system_runtime" },
								]}
							/>
							<NativeSelectField
								label="Status"
								value={status}
								onChange={setStatus}
								options={taskStatuses.map((value) => ({
									label: value || "Any",
									value,
								}))}
							/>
							<NativeSelectField
								label="Sort by"
								value={sortBy}
								onChange={(value) => setSortBy(value as AdminTaskSortBy)}
								options={[
									{ label: "updated_at", value: "updated_at" },
									{ label: "created_at", value: "created_at" },
									{ label: "id", value: "id" },
									{ label: "status", value: "status" },
									{ label: "progress", value: "progress" },
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
						<TextField
							label="Retry task ID"
							value={retryId}
							onChange={setRetryId}
						/>
						<TextField
							label="Cleanup finished before"
							type="datetime-local"
							value={finishedBefore}
							onChange={setFinishedBefore}
						/>
						<div className="flex flex-wrap gap-2">
							<Button
								type="button"
								variant="outline"
								onClick={() =>
									void listTask.run(() => adminTaskService.list(listParams()))
								}
							>
								<Icon name="ArrowsClockwise" className="size-4" />
								List
							</Button>
							<Button
								type="button"
								disabled={!hasRetryId}
								onClick={() =>
									void task.run(() => adminTaskService.retry(numericRetryId))
								}
							>
								<Icon name="Repeat" className="size-4" />
								Retry
							</Button>
							<Button
								type="button"
								variant="destructive"
								disabled={!cleanupDate}
								onClick={() =>
									void task.run(() =>
										adminTaskService.cleanup({
											finished_before: cleanupDate ?? "",
											kind: kind ? (kind as BackgroundTaskKind) : undefined,
											status: status
												? (status as BackgroundTaskStatus)
												: undefined,
										}),
									)
								}
							>
								<Icon name="Trash" className="size-4" />
								Cleanup
							</Button>
						</div>
					</CardContent>
				</Card>
				<JsonPanel
					title="Task action result"
					value={task.result}
					error={task.error}
					loading={task.loading}
				/>
			</div>

			<Card>
				<CardHeader className="border-b border-border/60 pb-4">
					<CardTitle>Background tasks</CardTitle>
				</CardHeader>
				<CardContent>
					{listTask.error ? (
						<JsonPanel title="List error" value={null} error={listTask.error} />
					) : (
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>ID</TableHead>
									<TableHead>Name</TableHead>
									<TableHead>Kind</TableHead>
									<TableHead>Creator</TableHead>
									<TableHead>Status</TableHead>
									<TableHead>Detail</TableHead>
									<TableHead>Progress</TableHead>
									<TableHead>Updated</TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{(listTask.result?.items ?? []).map((item) => (
									<TableRow key={item.id}>
										<TableCell>{item.id}</TableCell>
										<TableCell className="min-w-56 whitespace-normal">
											<div className="grid gap-1">
												<span className="font-medium">
													{formatTaskTitle(item)}
												</span>
												<span className="font-mono text-xs text-muted-foreground">
													{item.display_name}
												</span>
											</div>
										</TableCell>
										<TableCell>
											<div className="grid gap-1">
												<span>{formatTaskKind(item.kind)}</span>
												<span className="font-mono text-xs text-muted-foreground">
													{item.kind}
												</span>
											</div>
										</TableCell>
										<TableCell className="min-w-48 whitespace-normal">
											{item.creator ? (
												<div className="grid gap-1">
													<span>{item.creator.username}</span>
													<span className="text-xs text-muted-foreground">
														{item.creator.email}
													</span>
												</div>
											) : (
												<span className="text-muted-foreground">
													{item.creator_user_id ?? "system"}
												</span>
											)}
										</TableCell>
										<TableCell>
											<Badge variant={taskStatusBadgeVariant(item.status)}>
												{formatTaskStatusLabel(item.status)}
											</Badge>
										</TableCell>
										<TableCell className="max-w-80 whitespace-normal text-muted-foreground">
											{formatTaskStatusDetail(item)}
										</TableCell>
										<TableCell>{item.progress_percent}%</TableCell>
										<TableCell className="font-mono text-xs">
											{item.updated_at}
										</TableCell>
									</TableRow>
								))}
							</TableBody>
						</Table>
					)}
				</CardContent>
			</Card>

			<div className="space-y-3">
				<SectionTitle
					title="Tasks API coverage"
					description="Task list, cleanup, and retry are all callable from this page."
				/>
				<div className="grid gap-3 lg:grid-cols-2">
					{taskOps.map((item) => (
						<EndpointCard
							key={item.operationId}
							title={item.operationId}
							method={item.method}
							path={item.path}
							description={item.description}
							destructive={item.destructive}
						/>
					))}
				</div>
			</div>
		</PageShell>
	);
}
