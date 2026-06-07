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
import { compactRecord, emptyToNull, emptyToUndefined } from "@/lib/form";
import { adminExternalAuthService } from "@/services/adminService";
import type {
	AdminExternalAuthProviderPage,
	CreateExternalAuthProviderRequest,
	ExternalAuthKind,
	ExternalAuthProviderTestParamsRequest,
	UpdateExternalAuthProviderRequest,
} from "@/types/api";

export default function AdminExternalAuthPage() {
	const [id, setId] = useState("");
	const [displayName, setDisplayName] = useState("");
	const [clientId, setClientId] = useState("");
	const [clientSecret, setClientSecret] = useState("");
	const [slug, setSlug] = useState("");
	const [key, setKey] = useState("");
	const [kind, setKind] = useState<ExternalAuthKind>("oidc");
	const [enabled, setEnabled] = useState("true");
	const [issuerUrl, setIssuerUrl] = useState("");
	const [authorizeUrl, setAuthorizeUrl] = useState("");
	const [tokenUrl, setTokenUrl] = useState("");
	const [userinfoUrl, setUserinfoUrl] = useState("");
	const [scopes, setScopes] = useState("openid profile email");
	const task = useAsyncTask<unknown>();
	const listTask = useAsyncTask<AdminExternalAuthProviderPage>();
	const adminExternalOps = apiCatalog.filter(
		(item) => item.route === "/admin/external-auth",
	);
	const numericId = Number(id);
	const hasId = Number.isFinite(numericId) && numericId > 0;

	useEffect(() => {
		void listTask.run(() => adminExternalAuthService.list());
	}, [listTask.run]);

	function createBody(): CreateExternalAuthProviderRequest {
		return compactRecord({
			display_name: displayName,
			client_id: clientId,
			client_secret: emptyToNull(clientSecret),
			slug: emptyToNull(slug),
			key: emptyToNull(key),
			kind,
			provider_kind: kind,
			enabled: enabled === "true",
			issuer_url: emptyToNull(issuerUrl),
			authorize_url: emptyToNull(authorizeUrl),
			authorization_url: emptyToNull(authorizeUrl),
			token_url: emptyToNull(tokenUrl),
			userinfo_url: emptyToNull(userinfoUrl),
			scopes: emptyToNull(scopes),
		});
	}

	function updateBody(): UpdateExternalAuthProviderRequest {
		return compactRecord({
			display_name: emptyToUndefined(displayName),
			client_id: emptyToUndefined(clientId),
			client_secret: emptyToUndefined(clientSecret),
			slug: emptyToUndefined(slug),
			key: emptyToUndefined(key),
			kind,
			provider_kind: kind,
			enabled: enabled === "true",
			issuer_url: emptyToUndefined(issuerUrl),
			authorize_url: emptyToUndefined(authorizeUrl),
			authorization_url: emptyToUndefined(authorizeUrl),
			token_url: emptyToUndefined(tokenUrl),
			userinfo_url: emptyToUndefined(userinfoUrl),
			scopes: emptyToUndefined(scopes),
		});
	}

	function testBody(): ExternalAuthProviderTestParamsRequest {
		return compactRecord({
			client_id: clientId,
			client_secret: emptyToNull(clientSecret),
			kind,
			provider_kind: kind,
			issuer_url: emptyToNull(issuerUrl),
			authorize_url: emptyToNull(authorizeUrl),
			authorization_url: emptyToNull(authorizeUrl),
			token_url: emptyToNull(tokenUrl),
			userinfo_url: emptyToNull(userinfoUrl),
			scopes: emptyToNull(scopes),
		});
	}

	return (
		<PageShell
			title="Admin External Auth"
			description="Provider kind discovery, provider CRUD, parameter testing, and saved-provider testing."
		>
			<div className="grid gap-4 xl:grid-cols-[minmax(0,560px)_minmax(0,1fr)]">
				<Card>
					<CardHeader className="border-b border-border/60 pb-4">
						<CardTitle className="flex items-center gap-2">
							<Icon name="SignIn" className="size-4" />
							Provider editor
						</CardTitle>
						<CardDescription>
							Use test params before creating or patching providers.
						</CardDescription>
					</CardHeader>
					<CardContent className="grid gap-3">
						<div className="grid gap-3 sm:grid-cols-2">
							<TextField label="Provider ID" value={id} onChange={setId} />
							<NativeSelectField
								label="Kind"
								value={kind}
								onChange={(next) => setKind(next as ExternalAuthKind)}
								options={[
									{ label: "OIDC", value: "oidc" },
									{ label: "OAuth2", value: "oauth2" },
								]}
							/>
							<TextField
								label="Display name"
								value={displayName}
								onChange={setDisplayName}
							/>
							<TextField label="Slug" value={slug} onChange={setSlug} />
							<TextField label="Key" value={key} onChange={setKey} />
							<NativeSelectField
								label="Enabled"
								value={enabled}
								onChange={setEnabled}
								options={[
									{ label: "true", value: "true" },
									{ label: "false", value: "false" },
								]}
							/>
						</div>
						<TextField
							label="Client ID"
							value={clientId}
							onChange={setClientId}
						/>
						<TextField
							label="Client Secret"
							type="password"
							value={clientSecret}
							onChange={setClientSecret}
						/>
						<TextField
							label="Issuer URL"
							value={issuerUrl}
							onChange={setIssuerUrl}
						/>
						<TextField
							label="Authorize URL"
							value={authorizeUrl}
							onChange={setAuthorizeUrl}
						/>
						<div className="grid gap-3 sm:grid-cols-2">
							<TextField
								label="Token URL"
								value={tokenUrl}
								onChange={setTokenUrl}
							/>
							<TextField
								label="Userinfo URL"
								value={userinfoUrl}
								onChange={setUserinfoUrl}
							/>
						</div>
						<TextareaField
							label="Scopes"
							value={scopes}
							onChange={setScopes}
							rows={2}
						/>
						<div className="flex flex-wrap gap-2">
							<Button
								type="button"
								variant="outline"
								onClick={() =>
									void task.run(() => adminExternalAuthService.kinds())
								}
							>
								<Icon name="ListChecks" className="size-4" />
								Kinds
							</Button>
							<Button
								type="button"
								variant="outline"
								onClick={() =>
									void listTask.run(() => adminExternalAuthService.list())
								}
							>
								<Icon name="ArrowsClockwise" className="size-4" />
								List
							</Button>
							<Button
								type="button"
								disabled={!hasId}
								onClick={() =>
									void task.run(() => adminExternalAuthService.get(numericId))
								}
							>
								<Icon name="MagnifyingGlass" className="size-4" />
								Get
							</Button>
							<Button
								type="button"
								onClick={() =>
									void task.run(() =>
										adminExternalAuthService.testParams(testBody()),
									)
								}
							>
								<Icon name="Play" className="size-4" />
								Test params
							</Button>
							<Button
								type="button"
								disabled={!displayName || !clientId}
								onClick={() =>
									void task.run(() =>
										adminExternalAuthService.create(createBody()),
									)
								}
							>
								<Icon name="Plus" className="size-4" />
								Create
							</Button>
							<Button
								type="button"
								variant="outline"
								disabled={!hasId}
								onClick={() =>
									void task.run(() =>
										adminExternalAuthService.update(numericId, updateBody()),
									)
								}
							>
								<Icon name="PencilSimple" className="size-4" />
								Update
							</Button>
							<Button
								type="button"
								variant="outline"
								disabled={!hasId}
								onClick={() =>
									void task.run(() => adminExternalAuthService.test(numericId))
								}
							>
								<Icon name="Gauge" className="size-4" />
								Test saved
							</Button>
							<Button
								type="button"
								variant="destructive"
								disabled={!hasId}
								onClick={() =>
									void task.run(() =>
										adminExternalAuthService.delete(numericId),
									)
								}
							>
								<Icon name="Trash" className="size-4" />
								Delete
							</Button>
						</div>
					</CardContent>
				</Card>
				<JsonPanel
					title="Provider result"
					value={task.result}
					error={task.error}
					loading={task.loading}
				/>
			</div>

			<Card>
				<CardHeader className="border-b border-border/60 pb-4">
					<CardTitle>Configured providers</CardTitle>
				</CardHeader>
				<CardContent>
					{listTask.error ? (
						<JsonPanel title="List error" value={null} error={listTask.error} />
					) : (
						<Table>
							<TableHeader>
								<TableRow>
									<TableHead>ID</TableHead>
									<TableHead>Display</TableHead>
									<TableHead>Kind</TableHead>
									<TableHead>Slug</TableHead>
									<TableHead>Enabled</TableHead>
								</TableRow>
							</TableHeader>
							<TableBody>
								{(listTask.result?.items ?? []).map((item) => (
									<TableRow key={item.id}>
										<TableCell>{item.id}</TableCell>
										<TableCell>{item.display_name}</TableCell>
										<TableCell>{item.kind}</TableCell>
										<TableCell className="font-mono text-xs">
											{item.slug}
										</TableCell>
										<TableCell>{String(item.enabled)}</TableCell>
									</TableRow>
								))}
							</TableBody>
						</Table>
					)}
				</CardContent>
			</Card>

			<div className="space-y-3">
				<SectionTitle
					title="Admin External Auth API coverage"
					description="Every admin external-auth operation is represented here."
				/>
				<div className="grid gap-3 lg:grid-cols-2">
					{adminExternalOps.map((item) => (
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
