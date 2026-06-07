import type { IconName } from "@/components/ui/icon";
import type {
	AsterErrorCode,
	CheckResp,
	ExampleMessage,
	ExternalAuthPublicProvider,
	HealthResponse,
	ProtectedExampleMessage,
	SystemInfo,
} from "@/types/api";
import { ApiError, api, formatUnknownError } from "./http";

export type ServiceDiagnosticStatus =
	| "idle"
	| "loading"
	| "ok"
	| "guarded"
	| "error";

export type ServiceDiagnosticResult = {
	id: string;
	group: string;
	label: string;
	method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
	path: string;
	icon: IconName;
	status: ServiceDiagnosticStatus;
	value: string;
	detail?: string;
	error?: string;
};

type ServiceDiagnosticDefinition = Omit<
	ServiceDiagnosticResult,
	"status" | "value" | "detail" | "error"
> & {
	load: (signal: AbortSignal) => Promise<unknown>;
	summarize: (data: unknown) => string;
	describe?: (data: unknown) => string | undefined;
	guardedErrorCodes?: AsterErrorCode[];
	guardedValue?: string;
};

const definitions: ServiceDiagnosticDefinition[] = [
	{
		id: "health",
		group: "Runtime",
		label: "Process health",
		method: "GET",
		path: "/health",
		icon: "Gauge",
		load: (signal) => api.root.get<HealthResponse>("/health", { signal }),
		summarize: (data) => (data as HealthResponse).status,
		describe: (data) => `v${(data as HealthResponse).version}`,
	},
	{
		id: "ready",
		group: "Runtime",
		label: "Database readiness",
		method: "GET",
		path: "/health/ready",
		icon: "HardDrive",
		load: (signal) => api.root.get<HealthResponse>("/health/ready", { signal }),
		summarize: (data) => (data as HealthResponse).status,
		describe: (data) => `build ${(data as HealthResponse).build_time}`,
	},
	{
		id: "system",
		group: "Runtime",
		label: "Runtime information",
		method: "GET",
		path: "/api/v1/system/info",
		icon: "Cpu",
		load: (signal) => api.get<SystemInfo>("/system/info", { signal }),
		summarize: (data) => (data as SystemInfo).site_title,
		describe: (data) => {
			const info = data as SystemInfo;
			return `${info.name} ${info.version}`;
		},
	},
	{
		id: "auth-check",
		group: "Identity",
		label: "Auth bootstrap state",
		method: "GET",
		path: "/api/v1/auth/check",
		icon: "Key",
		load: (signal) => api.get<CheckResp>("/auth/check", { signal }),
		summarize: (data) =>
			(data as CheckResp).initialized ? "initialized" : "setup required",
		describe: () => "first-admin gate",
	},
	{
		id: "external-auth",
		group: "Identity",
		label: "External auth providers",
		method: "GET",
		path: "/api/v1/external-auth/providers",
		icon: "Globe",
		load: (signal) =>
			api.get<ExternalAuthPublicProvider[]>("/external-auth/providers", {
				signal,
			}),
		summarize: (data) => `${(data as ExternalAuthPublicProvider[]).length}`,
		describe: (data) => {
			const providers = data as ExternalAuthPublicProvider[];
			if (providers.length === 0) return "no enabled providers";
			return providers.map((provider) => provider.display_name).join(", ");
		},
	},
	{
		id: "public-example",
		group: "Examples",
		label: "Public template API",
		method: "GET",
		path: "/api/v1/examples/public",
		icon: "BracketsCurly",
		load: (signal) => api.get<ExampleMessage>("/examples/public", { signal }),
		summarize: (data) => (data as ExampleMessage).message,
		describe: (data) => `build ${(data as ExampleMessage).build_time}`,
	},
	{
		id: "protected-example",
		group: "Examples",
		label: "Authenticated API",
		method: "GET",
		path: "/api/v1/examples/protected",
		icon: "Lock",
		load: (signal) =>
			api.get<ProtectedExampleMessage>("/examples/protected", { signal }),
		summarize: (data) => (data as ProtectedExampleMessage).message,
		describe: (data) => (data as ProtectedExampleMessage).user.username,
		guardedErrorCodes: [
			"auth.token_invalid",
			"auth.token_expired",
			"forbidden",
		],
		guardedValue: "sign-in required",
	},
];

export const DIAGNOSTIC_ENDPOINTS = definitions.map(
	({ load, summarize, describe, guardedErrorCodes, guardedValue, ...meta }) =>
		meta,
);

export function createIdleDiagnostics(
	status: Extract<ServiceDiagnosticStatus, "idle" | "loading"> = "idle",
): ServiceDiagnosticResult[] {
	return definitions.map(({ load, summarize, describe, ...definition }) => ({
		...definition,
		status,
		value: status === "loading" ? "checking" : "not checked",
	}));
}

async function runDiagnostic(
	definition: ServiceDiagnosticDefinition,
	signal: AbortSignal,
): Promise<ServiceDiagnosticResult> {
	const {
		load,
		summarize,
		describe,
		guardedErrorCodes,
		guardedValue,
		...meta
	} = definition;

	try {
		const data = await load(signal);
		return {
			...meta,
			status: "ok",
			value: summarize(data),
			detail: describe?.(data),
		};
	} catch (error) {
		if (error instanceof ApiError && guardedErrorCodes?.includes(error.code)) {
			return {
				...meta,
				status: "guarded",
				value: guardedValue ?? "access controlled",
				detail: error.message,
			};
		}

		return {
			...meta,
			status: "error",
			value: "request failed",
			error: formatUnknownError(error),
		};
	}
}

export async function loadServiceDiagnostics(
	signal: AbortSignal,
): Promise<ServiceDiagnosticResult[]> {
	return Promise.all(
		definitions.map((definition) => runDiagnostic(definition, signal)),
	);
}
