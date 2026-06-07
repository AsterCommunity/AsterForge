import type {
	CheckResp,
	ExampleMessage,
	ExternalAuthPublicProvider,
	HealthResponse,
	ProtectedExampleMessage,
	SystemInfo,
} from "@/types/api";
import { api } from "./http";

export const systemService = {
	health: (signal?: AbortSignal) =>
		api.root.get<HealthResponse>("/health", { signal }),
	ready: (signal?: AbortSignal) =>
		api.root.get<HealthResponse>("/health/ready", { signal }),
	info: (signal?: AbortSignal) =>
		api.get<SystemInfo>("/system/info", { signal }),
	checkAuth: (signal?: AbortSignal) =>
		api.get<CheckResp>("/auth/check", { signal }),
	publicExternalAuthProviders: (signal?: AbortSignal) =>
		api.get<ExternalAuthPublicProvider[]>("/external-auth/providers", {
			signal,
		}),
	authExternalAuthProviders: (signal?: AbortSignal) =>
		api.get<ExternalAuthPublicProvider[]>("/auth/external-auth/providers", {
			signal,
		}),
	publicExample: (signal?: AbortSignal) =>
		api.get<ExampleMessage>("/examples/public", { signal }),
	protectedExample: (signal?: AbortSignal) =>
		api.get<ProtectedExampleMessage>("/examples/protected", { signal }),
};
