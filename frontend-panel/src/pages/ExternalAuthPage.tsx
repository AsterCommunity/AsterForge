import { useEffect, useState } from "react";
import { EndpointCard } from "@/components/panel/EndpointCard";
import { NativeSelectField, TextField } from "@/components/panel/FormControls";
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
import { useAsyncTask } from "@/hooks/useAsyncTask";
import { apiCatalog } from "@/lib/apiCatalog";
import { externalAuthService } from "@/services/externalAuthService";
import type { ExternalAuthKind } from "@/types/api";

function defaultRedirectUri() {
	if (typeof window === "undefined") return "http://localhost:5173/callback";
	return `${window.location.origin}/external-auth/callback`;
}

export default function ExternalAuthPage() {
	const [provider, setProvider] = useState("");
	const [kind, setKind] = useState<ExternalAuthKind>("oidc");
	const [redirectUri, setRedirectUri] = useState(defaultRedirectUri);
	const [state, setState] = useState("");
	const [code, setCode] = useState("");
	const task = useAsyncTask<unknown>();
	const externalOps = apiCatalog.filter(
		(item) => item.route === "/external-auth",
	);

	useEffect(() => {
		void task.run(async () => ({
			publicProviders: await externalAuthService.listPublic(),
			authProviders: await externalAuthService.listAuthAliases(),
		}));
	}, [task.run]);

	return (
		<PageShell
			title="External Auth"
			description="Public and auth-namespace external authentication provider discovery, login start, and callback finish."
		>
			<div className="grid gap-4 xl:grid-cols-[minmax(0,520px)_minmax(0,1fr)]">
				<Card>
					<CardHeader className="border-b border-border/60 pb-4">
						<CardTitle className="flex items-center gap-2">
							<Icon name="Globe" className="size-4" />
							Login flow probes
						</CardTitle>
						<CardDescription>
							Use a provider slug from the discovery response.
						</CardDescription>
					</CardHeader>
					<CardContent className="grid gap-3">
						<TextField
							label="Provider slug"
							value={provider}
							onChange={setProvider}
							placeholder="github"
						/>
						<NativeSelectField
							label="Kind"
							value={kind}
							onChange={(value) => setKind(value as ExternalAuthKind)}
							options={[
								{ label: "OIDC", value: "oidc" },
								{ label: "OAuth2", value: "oauth2" },
							]}
						/>
						<TextField
							label="Redirect URI"
							value={redirectUri}
							onChange={setRedirectUri}
						/>
						<div className="grid gap-3 sm:grid-cols-2">
							<TextField label="State" value={state} onChange={setState} />
							<TextField label="Code" value={code} onChange={setCode} />
						</div>
						<div className="flex flex-wrap gap-2">
							<Button
								type="button"
								variant="outline"
								onClick={() =>
									void task.run(() => externalAuthService.listPublic())
								}
							>
								<Icon name="List" className="size-4" />
								Public providers
							</Button>
							<Button
								type="button"
								variant="outline"
								onClick={() =>
									void task.run(() => externalAuthService.listAuthAliases())
								}
							>
								<Icon name="ListBullets" className="size-4" />
								Auth providers
							</Button>
							<Button
								type="button"
								variant="outline"
								onClick={() =>
									void task.run(() =>
										externalAuthService.listAuthAliasesByKind(kind),
									)
								}
							>
								<Icon name="MagnifyingGlass" className="size-4" />
								By kind
							</Button>
						</div>
						<div className="flex flex-wrap gap-2">
							<Button
								type="button"
								disabled={!provider}
								onClick={() =>
									void task.run(() =>
										externalAuthService.startPublic(provider, {
											redirect_uri: redirectUri,
										}),
									)
								}
							>
								<Icon name="Play" className="size-4" />
								Start public
							</Button>
							<Button
								type="button"
								disabled={!provider}
								onClick={() =>
									void task.run(() =>
										externalAuthService.startAuthAlias(kind, provider, {
											redirect_uri: redirectUri,
										}),
									)
								}
							>
								<Icon name="Play" className="size-4" />
								Start auth alias
							</Button>
							<Button
								type="button"
								variant="secondary"
								disabled={!provider || !state || !code}
								onClick={() =>
									void task.run(() =>
										externalAuthService.finishPublic(provider, state, code),
									)
								}
							>
								<Icon name="Check" className="size-4" />
								Finish public
							</Button>
							<Button
								type="button"
								variant="secondary"
								disabled={!provider || !state || !code}
								onClick={() =>
									void task.run(() =>
										externalAuthService.finishAuthAlias(
											kind,
											provider,
											state,
											code,
										),
									)
								}
							>
								<Icon name="Check" className="size-4" />
								Finish auth alias
							</Button>
						</div>
					</CardContent>
				</Card>

				<JsonPanel
					title="External auth result"
					value={task.result}
					error={task.error}
					loading={task.loading}
				/>
			</div>

			<div className="space-y-3">
				<SectionTitle
					title="External Auth API coverage"
					description="All public and /auth/external-auth endpoints have controls on this page."
				/>
				<div className="grid gap-3 lg:grid-cols-2">
					{externalOps.map((item) => (
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
