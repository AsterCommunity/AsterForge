import { useEffect, useState } from "react";
import { EndpointCard } from "@/components/panel/EndpointCard";
import {
	NativeSelectField,
	TextareaField,
	TextField,
} from "@/components/panel/FormControls";
import { JsonPanel } from "@/components/panel/JsonPanel";
import { PageShell, SectionTitle } from "@/components/panel/PageShell";
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
import { parseStringOrStringArray } from "@/lib/form";
import { adminConfigService } from "@/services/adminService";
import type { SystemConfigPage, SystemConfigVisibility } from "@/types/api";

export default function AdminConfigPage() {
	const [key, setKey] = useState("");
	const [value, setValue] = useState("");
	const [visibility, setVisibility] =
		useState<SystemConfigVisibility>("private");
	const task = useAsyncTask<unknown>();
	const listTask = useAsyncTask<SystemConfigPage>();
	const configOps = apiCatalog.filter((item) => item.route === "/admin/config");

	useEffect(() => {
		void listTask.run(() => adminConfigService.list());
	}, [listTask.run]);

	return (
		<PageShell
			title="Admin Config"
			description="Runtime config listing, schema inspection, single-key lookup, set, and delete."
		>
			<div className="grid gap-4 xl:grid-cols-[minmax(0,520px)_minmax(0,1fr)]">
				<Card>
					<CardHeader className="border-b border-border/60 pb-4">
						<CardTitle className="flex items-center gap-2">
							<Icon name="Gear" className="size-4" />
							Config editor
						</CardTitle>
						<CardDescription>
							Values accept plain text, JSON string, or JSON/string-array lines.
						</CardDescription>
					</CardHeader>
					<CardContent className="grid gap-3">
						<TextField
							label="Config key"
							value={key}
							onChange={setKey}
							placeholder="site.title"
						/>
						<TextareaField
							label="Value"
							value={value}
							onChange={setValue}
							rows={5}
							placeholder={'AsterForge\nor ["value-a", "value-b"]'}
						/>
						<NativeSelectField
							label="Visibility"
							value={visibility}
							onChange={(next) => setVisibility(next as SystemConfigVisibility)}
							options={[
								{ label: "private", value: "private" },
								{ label: "authenticated", value: "authenticated" },
								{ label: "public", value: "public" },
							]}
						/>
						<div className="flex flex-wrap gap-2">
							<Button
								type="button"
								variant="outline"
								onClick={() => void task.run(() => adminConfigService.schema())}
							>
								<Icon name="ListChecks" className="size-4" />
								Schema
							</Button>
							<Button
								type="button"
								variant="outline"
								onClick={() =>
									void listTask.run(() => adminConfigService.list())
								}
							>
								<Icon name="ArrowsClockwise" className="size-4" />
								List
							</Button>
							<Button
								type="button"
								disabled={!key.trim()}
								onClick={() =>
									void task.run(() => adminConfigService.get(key.trim()))
								}
							>
								<Icon name="MagnifyingGlass" className="size-4" />
								Get
							</Button>
							<Button
								type="button"
								disabled={!key.trim()}
								onClick={() =>
									void task.run(() =>
										adminConfigService.set(key.trim(), {
											value: parseStringOrStringArray(value),
											visibility,
										}),
									)
								}
							>
								<Icon name="FloppyDisk" className="size-4" />
								Set
							</Button>
							<Button
								type="button"
								variant="destructive"
								disabled={!key.trim()}
								onClick={() =>
									void task.run(() => adminConfigService.delete(key.trim()))
								}
							>
								<Icon name="Trash" className="size-4" />
								Delete
							</Button>
						</div>
					</CardContent>
				</Card>
				<JsonPanel
					title="Config result"
					value={task.result}
					error={task.error}
					loading={task.loading}
				/>
			</div>

			<Card>
				<CardHeader className="border-b border-border/60 pb-4">
					<CardTitle>Config entries</CardTitle>
				</CardHeader>
				<CardContent>
					{listTask.error ? (
						<JsonPanel title="List error" value={null} error={listTask.error} />
					) : (
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>Key</TableHead>
									<TableHead>Category</TableHead>
									<TableHead>Visibility</TableHead>
									<TableHead>Source</TableHead>
									<TableHead>Value</TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{(listTask.result?.items ?? []).map((item) => (
									<TableRow key={item.id}>
										<TableCell className="font-mono text-xs">
											{item.key}
										</TableCell>
										<TableCell>{item.category}</TableCell>
										<TableCell>{item.visibility}</TableCell>
										<TableCell>{item.source}</TableCell>
										<TableCell className="max-w-96 truncate font-mono text-xs">
											{JSON.stringify(item.value)}
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
					title="Config API coverage"
					description="Every admin config operation is wired here."
				/>
				<div className="grid gap-3 lg:grid-cols-2">
					{configOps.map((item) => (
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
