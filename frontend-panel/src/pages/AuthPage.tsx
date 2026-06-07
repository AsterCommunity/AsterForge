import { type FormEvent, useState } from "react";
import { EndpointCard } from "@/components/panel/EndpointCard";
import { NativeSelectField, TextField } from "@/components/panel/FormControls";
import { JsonPanel } from "@/components/panel/JsonPanel";
import { PageShell, SectionTitle } from "@/components/panel/PageShell";
import { Button } from "@/components/ui/button";
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
import { useAsyncTask } from "@/hooks/useAsyncTask";
import { apiCatalog } from "@/lib/apiCatalog";
import { authService } from "@/services/authService";
import { useAuthStore } from "@/stores/authStore";

type AuthMode = "login" | "register" | "setup";

export default function AuthPage() {
	const [mode, setMode] = useState<AuthMode>("login");
	const [username, setUsername] = useState("admin");
	const [email, setEmail] = useState("admin@example.com");
	const [identifier, setIdentifier] = useState("admin");
	const [password, setPassword] = useState("");
	const task = useAsyncTask<unknown>();
	const user = useAuthStore((state) => state.user);
	const isAuthenticated = useAuthStore((state) => state.isAuthenticated);
	const setup = useAuthStore((state) => state.setup);
	const register = useAuthStore((state) => state.register);
	const login = useAuthStore((state) => state.login);
	const refresh = useAuthStore((state) => state.refresh);
	const logout = useAuthStore((state) => state.logout);
	const clear = useAuthStore((state) => state.clear);
	const authOps = apiCatalog.filter((item) => item.route === "/auth");

	async function submit(event: FormEvent<HTMLFormElement>) {
		event.preventDefault();
		await task.run(async () => {
			if (mode === "setup") {
				await setup(username, email, password);
				return {
					action: "setup_first_admin",
					user: useAuthStore.getState().user,
				};
			}
			if (mode === "register") {
				await register(username, email, password);
				return { action: "register", user: useAuthStore.getState().user };
			}
			await login(identifier, password);
			return { action: "login", user: useAuthStore.getState().user };
		});
	}

	return (
		<PageShell
			title="Auth"
			description="Local setup, registration, login, cookie refresh, logout, current user, and session APIs."
		>
			<div className="grid gap-4 xl:grid-cols-[minmax(0,520px)_minmax(0,1fr)]">
				<Card>
					<CardHeader className="border-b border-border/60 pb-4">
						<CardTitle className="flex items-center gap-2">
							<Icon name="Key" className="size-4" />
							Session control
						</CardTitle>
					</CardHeader>
					<CardContent>
						<form className="grid gap-3" onSubmit={submit}>
							<NativeSelectField
								label="Mode"
								value={mode}
								onChange={(value) => setMode(value as AuthMode)}
								options={[
									{ label: "Login", value: "login" },
									{ label: "Register", value: "register" },
									{ label: "Setup first admin", value: "setup" },
								]}
							/>
							{mode === "login" ? (
								<TextField
									label="Identifier"
									value={identifier}
									onChange={setIdentifier}
									required
								/>
							) : (
								<>
									<TextField
										label="Username"
										value={username}
										onChange={setUsername}
										required
									/>
									<TextField
										label="Email"
										type="email"
										value={email}
										onChange={setEmail}
										required
									/>
								</>
							)}
							<TextField
								label="Password"
								type="password"
								value={password}
								onChange={setPassword}
								required
							/>
							<div className="flex flex-wrap gap-2">
								<Button type="submit" disabled={task.loading}>
									<Icon
										name={task.loading ? "Spinner" : "SignIn"}
										className={task.loading ? "size-4 animate-spin" : "size-4"}
									/>
									Submit
								</Button>
								<Button
									type="button"
									variant="outline"
									onClick={() => void task.run(() => authService.check())}
								>
									<Icon name="Eye" className="size-4" />
									Check
								</Button>
								<Button
									type="button"
									variant="outline"
									disabled={!isAuthenticated}
									onClick={() =>
										void task.run(async () => {
											await refresh();
											return {
												action: "refresh_token",
												user: useAuthStore.getState().user,
											};
										})
									}
								>
									<Icon name="ArrowsClockwise" className="size-4" />
									Refresh
								</Button>
								<Button
									type="button"
									variant="outline"
									disabled={!isAuthenticated}
									onClick={() =>
										void task.run(async () => {
											await logout();
											return { action: "logout" };
										})
									}
								>
									<Icon name="SignOut" className="size-4" />
									Logout
								</Button>
								<Button type="button" variant="ghost" onClick={clear}>
									<Icon name="X" className="size-4" />
									Clear local
								</Button>
							</div>
						</form>
					</CardContent>
				</Card>

				<div className="grid gap-4">
					<JsonPanel
						title="Auth result"
						value={task.result}
						error={task.error}
						loading={task.loading}
					/>
					<Card size="sm">
						<CardHeader>
							<CardTitle>Current cookie session</CardTitle>
						</CardHeader>
						<CardContent className="grid gap-2 text-sm">
							<div>User: {user?.username ?? "-"}</div>
							<div>Role: {user?.role ?? "-"}</div>
							<div>Authenticated: {isAuthenticated ? "yes" : "no"}</div>
							<div>Credential storage: HttpOnly cookies</div>
						</CardContent>
					</Card>
				</div>
			</div>

			<div className="grid gap-3 lg:grid-cols-2">
				<EndpointCard
					title="get_current_user"
					method="GET"
					path="/api/v1/auth/me"
					actionLabel="Load me"
					onAction={() => void task.run(() => authService.me())}
				/>
				<EndpointCard
					title="list_auth_sessions"
					method="GET"
					path="/api/v1/auth/sessions"
					actionLabel="Load sessions"
					onAction={() => void task.run(() => authService.sessions())}
				/>
			</div>

			<div className="space-y-3">
				<SectionTitle
					title="Auth API coverage"
					description="All local auth endpoints have direct controls on this page; browser credentials are sent with cookies."
				/>
				<Table>
					<TableHeader>
						<TableRow>
							<TableHead>Operation</TableHead>
							<TableHead>Method</TableHead>
							<TableHead>Path</TableHead>
							<TableHead>Access</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{authOps.map((item) => (
							<TableRow key={item.operationId}>
								<TableCell className="font-mono text-xs">
									{item.operationId}
								</TableCell>
								<TableCell>{item.method}</TableCell>
								<TableCell className="font-mono text-xs">{item.path}</TableCell>
								<TableCell>{item.access}</TableCell>
							</TableRow>
						))}
					</TableBody>
				</Table>
			</div>
		</PageShell>
	);
}
