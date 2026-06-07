import { withQuery } from "@/lib/query";
import type {
	AdminAuditLogQuery,
	AdminExternalAuthProviderInfo,
	AdminExternalAuthProviderPage,
	AdminTaskCleanupRequest,
	AdminTaskListQuery,
	AdminTaskPage,
	AuditLogPage,
	ConfigSchemaItem,
	CreateExternalAuthProviderRequest,
	ExternalAuthProviderKindInfo,
	ExternalAuthProviderTestParamsRequest,
	ExternalAuthProviderTestResult,
	RemovedCountResponse,
	SetConfigRequest,
	SystemConfig,
	SystemConfigPage,
	UpdateExternalAuthProviderRequest,
} from "@/types/api";
import { api } from "./http";

export const adminAuditService = {
	list: (params: AdminAuditLogQuery = {}) =>
		api.get<AuditLogPage>(
			withQuery("/admin/audit-logs", {
				limit: params.limit ?? 50,
				offset: params.offset ?? 0,
				user_id: params.user_id,
				action: params.action,
				entity_type: params.entity_type,
				entity_id: params.entity_id,
				after: params.after,
				before: params.before,
				sort_by: params.sort_by ?? "created_at",
				sort_order: params.sort_order ?? "desc",
			}),
		),
};

export const adminConfigService = {
	list: (params: { limit?: number; offset?: number } = {}) =>
		api.get<SystemConfigPage>(
			withQuery("/admin/config", {
				limit: params.limit ?? 50,
				offset: params.offset ?? 0,
			}),
		),
	schema: () => api.get<ConfigSchemaItem[]>("/admin/config/schema"),
	get: (key: string) =>
		api.get<SystemConfig>(`/admin/config/${encodeURIComponent(key)}`),
	set: (key: string, data: SetConfigRequest) =>
		api.put<SystemConfig>(`/admin/config/${encodeURIComponent(key)}`, data),
	delete: (key: string) =>
		api.delete<void>(`/admin/config/${encodeURIComponent(key)}`),
};

export const adminTaskService = {
	list: (params: AdminTaskListQuery = {}) =>
		api.get<AdminTaskPage>(
			withQuery("/admin/tasks", {
				limit: params.limit ?? 50,
				offset: params.offset ?? 0,
				kind: params.kind,
				status: params.status,
				sort_by: params.sort_by ?? "updated_at",
				sort_order: params.sort_order ?? "desc",
			}),
		),
	cleanup: (data: AdminTaskCleanupRequest) =>
		api.post<RemovedCountResponse>("/admin/tasks/cleanup", data),
	retry: (id: number) =>
		api.post<AdminTaskPage["items"][number]>(`/admin/tasks/${id}/retry`),
};

export const adminExternalAuthService = {
	kinds: () =>
		api.get<ExternalAuthProviderKindInfo[]>(
			"/admin/external-auth/provider-kinds",
		),
	list: (params: { limit?: number; offset?: number } = {}) =>
		api.get<AdminExternalAuthProviderPage>(
			withQuery("/admin/external-auth/providers", {
				limit: params.limit ?? 50,
				offset: params.offset ?? 0,
			}),
		),
	get: (id: number) =>
		api.get<AdminExternalAuthProviderInfo>(
			`/admin/external-auth/providers/${id}`,
		),
	create: (data: CreateExternalAuthProviderRequest) =>
		api.post<AdminExternalAuthProviderInfo>(
			"/admin/external-auth/providers",
			data,
		),
	update: (id: number, data: UpdateExternalAuthProviderRequest) =>
		api.patch<AdminExternalAuthProviderInfo>(
			`/admin/external-auth/providers/${id}`,
			data,
		),
	delete: (id: number) =>
		api.delete<void>(`/admin/external-auth/providers/${id}`),
	testParams: (data: ExternalAuthProviderTestParamsRequest) =>
		api.post<ExternalAuthProviderTestResult>(
			"/admin/external-auth/providers/test",
			data,
		),
	test: (id: number) =>
		api.post<ExternalAuthProviderTestResult>(
			`/admin/external-auth/providers/${id}/test`,
		),
};
