// Re-export generated API types for convenience.
// Import from this file instead of api.generated.ts so code is not coupled to the generator output shape.
import type {
	operations as ApiOperations,
	components,
} from "@/types/api.generated";

export type { operations, paths } from "@/types/api.generated";

export type OperationData<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends {
		responses: {
			200: {
				content: {
					"application/json": {
						data?: infer Data;
					};
				};
			};
		};
	}
		? NonNullable<Data>
		: never;

export type OperationQuery<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends { parameters: { query?: infer Query } }
		? NonNullable<Query>
		: never;

export type OperationPath<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends { parameters: { path: infer Path } }
		? NonNullable<Path>
		: never;

export type OperationRequestBody<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends {
		requestBody: {
			content: {
				"application/json": infer Body;
			};
		};
	}
		? NonNullable<Body>
		: never;

export type AsterErrorCode = components["schemas"]["AsterErrorCode"];
export type ApiErrorInfo = components["schemas"]["ApiErrorInfo"];

export type ApiResponse<T = unknown> = {
	code: AsterErrorCode;
	msg: string;
	data?: T | null;
	error?: ApiErrorInfo | null;
};

export type AdminAuditLogSortBy = components["schemas"]["AdminAuditLogSortBy"];
export type AdminAuditLogQuery = OperationQuery<"list_audit_logs">;
export type AuditAction = components["schemas"]["AuditAction"];
export type AuditEntityType = components["schemas"]["AuditEntityType"];
export type AuditLogEntry = components["schemas"]["AuditLogEntry"];
export type AuditLogPage = components["schemas"]["OffsetPage_AuditLogEntry"];
export type AuditPresentation = components["schemas"]["AuditPresentation"];
export type AuditPresentationMessage =
	components["schemas"]["AuditPresentationMessage"];
export type AuthTokenResponse = components["schemas"]["AuthTokenResponse"];
export type AuthUserInfo = components["schemas"]["AuthUserInfo"];
export type AdminExternalAuthProviderInfo =
	components["schemas"]["AdminExternalAuthProviderInfo"];
export type AdminExternalAuthProviderPage =
	components["schemas"]["OffsetPage_AdminExternalAuthProviderInfo"];
export type AdminTaskCleanupRequest =
	components["schemas"]["AdminTaskCleanupReq"];
export type AdminTaskListQuery = OperationQuery<"admin_list_tasks">;
export type AdminTaskSortBy = components["schemas"]["AdminTaskSortBy"];
export type AdminTaskPage = components["schemas"]["OffsetPage_TaskInfo"];
export type BackgroundTaskKind = components["schemas"]["BackgroundTaskKind"];
export type BackgroundTaskStatus =
	components["schemas"]["BackgroundTaskStatus"];
export type CheckResp = components["schemas"]["CheckResp"];
export type ConfigSchemaItem = components["schemas"]["ConfigSchemaItem"];
export type CreateExternalAuthProviderRequest =
	components["schemas"]["CreateExternalAuthProviderReq"];
export type ExampleMessage = components["schemas"]["ExampleMessage"];
export type ExternalAuthProviderKindInfo =
	components["schemas"]["ExternalAuthProviderKindInfo"];
export type ExternalAuthProviderTestParamsRequest =
	components["schemas"]["ExternalAuthProviderTestParamsReq"];
export type ExternalAuthProviderTestResult =
	components["schemas"]["ExternalAuthProviderTestResult"];
export type ExternalAuthKind = components["schemas"]["ExternalAuthKind"];
export type ExternalAuthPublicProvider =
	components["schemas"]["ExternalAuthPublicProvider"];
export type ExternalAuthStartLoginRequest =
	components["schemas"]["StartExternalAuthReq"];
export type ExternalAuthStartLoginResponse =
	components["schemas"]["ExternalAuthStartLoginResponse"];
export type ExternalAuthFinishLoginResponse =
	components["schemas"]["ExternalAuthFinishLoginResponse"];
export type HealthResponse = components["schemas"]["HealthResponse"];
export type LoginRequest = components["schemas"]["LoginReq"];
export type LogoutRequest = components["schemas"]["LogoutReq"];
export type LogoutResponse = components["schemas"]["LogoutResp"];
export type ProtectedExampleMessage =
	components["schemas"]["ProtectedExampleMessage"];
export type RefreshRequest = components["schemas"]["RefreshReq"];
export type RegisterRequest = components["schemas"]["RegisterReq"];
export type RemovedCountResponse =
	components["schemas"]["RemovedCountResponse"];
export type SetConfigRequest = components["schemas"]["SetConfigReq"];
export type SetupRequest = components["schemas"]["SetupReq"];
export type SortOrder = components["schemas"]["SortOrder"];
export type SystemConfig = components["schemas"]["SystemConfig"];
export type SystemConfigPage = components["schemas"]["OffsetPage_SystemConfig"];
export type SystemConfigValue = components["schemas"]["SystemConfigValue"];
export type SystemConfigVisibility =
	components["schemas"]["SystemConfigVisibility"];
export type SystemInfo = components["schemas"]["SystemInfo"];
export type TaskInfo = components["schemas"]["TaskInfo"];
export type TaskCreatorSummary = components["schemas"]["TaskCreatorSummary"];
export type TaskPresentation = components["schemas"]["TaskPresentation"];
export type TaskPresentationCode =
	components["schemas"]["TaskPresentationCode"];
export type TaskPresentationMessage =
	components["schemas"]["TaskPresentationMessage"];
export type UpdateExternalAuthProviderRequest =
	components["schemas"]["UpdateExternalAuthProviderReq"];
export type UserRole = components["schemas"]["UserRole"];
export type UserStatus = components["schemas"]["UserStatus"];
export type AuthSessionInfo = OperationData<"list_auth_sessions">[number];
