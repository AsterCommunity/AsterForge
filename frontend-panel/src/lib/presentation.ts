import type {
	AuditAction,
	AuditEntityType,
	AuditLogEntry,
	AuditPresentationMessage,
	BackgroundTaskStatus,
	TaskInfo,
	TaskPresentationCode,
	TaskPresentationMessage,
} from "@/types/api";

const TASK_TITLE_LABELS = {
	runtime_task_audit_cleanup: "Audit log cleanup",
	runtime_task_auth_session_cleanup: "Auth session cleanup",
	runtime_task_background_task_dispatch: "Background task dispatch",
	runtime_task_external_auth_flow_cleanup: "External auth flow cleanup",
	runtime_task_mail_outbox_dispatch: "Mail outbox dispatch",
	runtime_task_system_health_check: "System health check",
	runtime_task_task_cleanup: "Task artifact cleanup",
	status_text_failed: "Failed",
	status_text_quiet: "No changes",
	status_text_succeeded: "Succeeded",
	status_text_system_healthy: "System healthy",
	runtime_system_health_issue_detail: "System health issue",
} as const satisfies Record<TaskPresentationCode, string>;

const TASK_STATUS_LABELS = {
	pending: "Pending",
	processing: "Processing",
	retry: "Retrying",
	succeeded: "Succeeded",
	failed: "Failed",
	canceled: "Canceled",
} as const satisfies Record<BackgroundTaskStatus, string>;

const AUDIT_ACTION_LABELS = {
	admin_cleanup_tasks: "Tasks cleaned up",
	admin_create_external_auth_provider: "External auth provider created",
	admin_create_user: "User created",
	admin_delete_config: "Config deleted",
	admin_delete_external_auth_provider: "External auth provider deleted",
	admin_disable_user: "User disabled",
	admin_revoke_user_sessions: "User sessions revoked",
	admin_test_external_auth_provider: "External auth provider tested",
	admin_update_external_auth_provider: "External auth provider updated",
	admin_update_user: "User updated",
	config_action_execute: "Config action executed",
	config_delete: "Config deleted",
	config_update: "Config updated",
	external_auth_provider_create: "External auth provider created",
	external_auth_provider_delete: "External auth provider deleted",
	external_auth_provider_update: "External auth provider updated",
	mail_delivery_failed: "Email delivery failed",
	mail_send: "Email sent",
	server_shutdown: "Server stopped",
	server_start: "Server started",
	system_setup: "System setup",
	task_retry: "Task retry scheduled",
	user_change_password: "Password changed",
	user_external_auth_link: "External auth linked",
	user_external_auth_login: "External auth login",
	user_external_auth_unlink: "External auth unlinked",
	user_login: "User login",
	user_logout: "User logout",
	user_refresh_token: "Session refreshed",
	user_register: "User registered",
	user_revoke_other_sessions: "Other sessions revoked",
	user_revoke_session: "Session revoked",
	user_update_profile: "Profile updated",
} as const satisfies Record<AuditAction, string>;

const AUDIT_ENTITY_LABELS = {
	api_token: "API token",
	auth_session: "Auth session",
	external_auth_identity: "External auth identity",
	external_auth_provider: "External auth provider",
	mail: "Mail",
	server: "Server",
	system: "System",
	system_config: "System config",
	task: "Task",
	user: "User",
} as const satisfies Record<AuditEntityType | "server", string>;

const AUDIT_DETAIL_LABELS = {
	config_action_executed: "Config action executed",
	config_value_updated: "Config value updated",
	external_auth_provider_changed: "Provider settings changed",
	external_auth_provider_tested: "Provider connection tested",
	mail_delivery_failed: "Email delivery failed",
	mail_sent: "Email sent",
	task_retry_scheduled: "Retry queued",
	tasks_cleanup_finished: "Cleanup finished",
	user_login_identifier: "Login identifier",
} as const;

type AuditDetailCode = keyof typeof AUDIT_DETAIL_LABELS;

function isRecord(value: unknown): value is Record<string, unknown> {
	return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function messageParams(
	message:
		| AuditPresentationMessage
		| TaskPresentationMessage
		| null
		| undefined,
) {
	return isRecord(message?.params) ? message.params : {};
}

function valueText(value: unknown): string {
	if (typeof value === "string") return value.trim();
	if (typeof value === "number" || typeof value === "boolean") {
		return String(value);
	}
	if (Array.isArray(value)) {
		const values: string[] = value.map(valueText).filter(Boolean);
		return values.length > 0 ? values.join(", ") : "";
	}
	if (isRecord(value)) {
		return JSON.stringify(value);
	}
	return "";
}

function paramText(params: Record<string, unknown>, key: string) {
	return valueText(params[key]);
}

function humanizeCode(value: string) {
	const words = value.replaceAll("-", "_").split("_").filter(Boolean);
	if (words.length === 0) return value;
	const text = words.join(" ");
	return text.charAt(0).toUpperCase() + text.slice(1);
}

function labelForCode<T extends string>(
	labels: Partial<Record<T, string>>,
	code: string,
) {
	return labels[code as T] ?? humanizeCode(code);
}

function compactJoin(
	values: Array<string | null | undefined>,
	separator = " · ",
) {
	return values
		.filter((value): value is string => Boolean(value))
		.join(separator);
}

function formatKeyValues(
	params: Record<string, unknown>,
	keys: Array<[string, string]>,
) {
	return keys
		.map(([key, label]) => {
			const value = paramText(params, key);
			return value ? `${label}: ${value}` : null;
		})
		.filter((value): value is string => Boolean(value))
		.join("; ");
}

function formatRuntimeHealthComponent(component: unknown) {
	if (!isRecord(component)) return null;
	const name = paramText(component, "name");
	const status = paramText(component, "status");
	if (!name && !status) return null;
	const message = paramText(component, "message");
	const summary = compactJoin(
		[name ? humanizeCode(name) : null, status ? humanizeCode(status) : null],
		" ",
	);
	return message ? `${summary}: ${message}` : summary;
}

function formatRuntimeHealthIssue(message: TaskPresentationMessage) {
	const params = messageParams(message);
	const components = Array.isArray(params.components)
		? params.components
				.map(formatRuntimeHealthComponent)
				.filter((component): component is string => Boolean(component))
		: [];
	if (components.length > 0) {
		return components.join("; ");
	}
	const status = paramText(params, "status");
	return status ? humanizeCode(status) : TASK_TITLE_LABELS[message.code];
}

function formatTaskPresentationMessage(
	message: TaskPresentationMessage | null | undefined,
	fallback: string,
) {
	if (!message?.code) return fallback;
	if (message.code === "runtime_system_health_issue_detail") {
		return formatRuntimeHealthIssue(message) || fallback;
	}
	if (message.code === "status_text_failed") {
		const error = paramText(messageParams(message), "error");
		return error ? `Failed: ${error}` : TASK_TITLE_LABELS[message.code];
	}
	return TASK_TITLE_LABELS[message.code] ?? fallback;
}

export function formatTaskTitle(
	task: Pick<TaskInfo, "display_name" | "presentation">,
) {
	return formatTaskPresentationMessage(
		task.presentation?.title,
		task.display_name,
	);
}

export function formatTaskStatusLabel(status: BackgroundTaskStatus) {
	return TASK_STATUS_LABELS[status] ?? humanizeCode(status);
}

export function formatTaskKind(kind: string) {
	return humanizeCode(kind);
}

export function formatTaskStatusDetail(
	task: Pick<TaskInfo, "last_error" | "presentation" | "status_text">,
) {
	const lastError = task.last_error?.trim();
	if (lastError) return lastError;

	const presentation = task.presentation?.status
		? formatTaskPresentationMessage(task.presentation.status, "")
		: "";
	if (presentation) return presentation;

	const statusText = task.status_text?.trim();
	return statusText || "-";
}

export function taskStatusBadgeVariant(status: BackgroundTaskStatus) {
	switch (status) {
		case "failed":
			return "destructive";
		case "canceled":
			return "outline";
		case "succeeded":
			return "default";
		case "pending":
		case "processing":
		case "retry":
			return "secondary";
	}
}

export function formatAuditAction(action: AuditAction | string) {
	return labelForCode<AuditAction>(AUDIT_ACTION_LABELS, String(action));
}

export function formatAuditEntityType(
	entityType: AuditEntityType | "server" | string,
) {
	return labelForCode<AuditEntityType | "server">(
		AUDIT_ENTITY_LABELS,
		String(entityType),
	);
}

function formatAuditTargetMessage(
	message: AuditPresentationMessage | null | undefined,
	fallback: string,
) {
	if (!message?.code) return fallback;
	const params = messageParams(message);
	const name = paramText(params, "name");
	const id = paramText(params, "id");
	const type = formatAuditEntityType(message.code);
	return compactJoin([
		name || (id ? null : fallback),
		type,
		id ? `#${id}` : null,
	]);
}

function formatAuditDetailMessage(
	message: AuditPresentationMessage | null | undefined,
) {
	if (!message?.code) return null;
	const params = messageParams(message);

	switch (message.code as AuditDetailCode) {
		case "config_value_updated": {
			const values = formatKeyValues(params, [
				["value", "Value"],
				["prior_visibility", "Previous visibility"],
				["visibility", "Visibility"],
			]);
			return compactJoin([AUDIT_DETAIL_LABELS.config_value_updated, values]);
		}
		case "config_action_executed": {
			const action = paramText(params, "action");
			const values = formatKeyValues(params, [
				["target_email", "Target email"],
			]);
			return compactJoin([
				action
					? `Config action: ${humanizeCode(action)}`
					: AUDIT_DETAIL_LABELS.config_action_executed,
				values,
			]);
		}
		case "external_auth_provider_changed": {
			const values = formatKeyValues(params, [
				["key", "Key"],
				["slug", "Slug"],
				["kind", "Kind"],
				["issuer_url", "Issuer"],
				["enabled", "Enabled"],
			]);
			return compactJoin([
				AUDIT_DETAIL_LABELS.external_auth_provider_changed,
				values,
			]);
		}
		case "external_auth_provider_tested": {
			const success = paramText(params, "success");
			const status = success ? (success === "true" ? "passed" : "failed") : "";
			const values = formatKeyValues(params, [
				["provider", "Provider"],
				["key", "Key"],
				["slug", "Slug"],
				["kind", "Kind"],
				["issuer_url", "Issuer"],
				["enabled", "Enabled"],
			]);
			return compactJoin([
				status
					? `Provider test ${status}`
					: AUDIT_DETAIL_LABELS.external_auth_provider_tested,
				values,
			]);
		}
		case "task_retry_scheduled": {
			const values = formatKeyValues(params, [
				["kind", "Kind"],
				["previous_attempt_count", "Previous attempts"],
			]);
			return compactJoin([AUDIT_DETAIL_LABELS.task_retry_scheduled, values]);
		}
		case "mail_sent": {
			const values = formatKeyValues(params, [
				["template_code", "Template"],
				["to_address", "To"],
				["outbox_id", "Outbox"],
			]);
			return compactJoin([AUDIT_DETAIL_LABELS.mail_sent, values]);
		}
		case "mail_delivery_failed": {
			const values = formatKeyValues(params, [
				["template_code", "Template"],
				["to_address", "To"],
				["outbox_id", "Outbox"],
				["attempt_count", "Attempts"],
				["error", "Error"],
			]);
			return compactJoin([AUDIT_DETAIL_LABELS.mail_delivery_failed, values]);
		}
		case "tasks_cleanup_finished": {
			const removed = paramText(params, "removed");
			const values = formatKeyValues(params, [
				["finished_before", "Before"],
				["kind", "Kind"],
				["status", "Status"],
			]);
			return compactJoin([
				removed
					? `Removed ${removed} tasks`
					: AUDIT_DETAIL_LABELS.tasks_cleanup_finished,
				values,
			]);
		}
		case "user_login_identifier": {
			const identifier = paramText(params, "identifier");
			return identifier
				? `Identifier: ${identifier}`
				: AUDIT_DETAIL_LABELS.user_login_identifier;
		}
		default:
			return labelForCode<AuditDetailCode>(AUDIT_DETAIL_LABELS, message.code);
	}
}

export function formatAuditSummary(
	entry: Pick<AuditLogEntry, "action" | "presentation">,
) {
	const message = entry.presentation?.summary;
	if (!message?.code) return formatAuditAction(entry.action);
	return formatAuditAction(message.code);
}

export function formatAuditTarget(
	entry: Pick<
		AuditLogEntry,
		"entity_id" | "entity_name" | "entity_type" | "presentation"
	>,
) {
	const fallback =
		entry.entity_name ?? formatAuditEntityType(entry.entity_type);
	if (entry.presentation?.target) {
		return formatAuditTargetMessage(entry.presentation.target, fallback);
	}
	return compactJoin([
		entry.entity_name ?? null,
		formatAuditEntityType(entry.entity_type),
		entry.entity_id == null ? null : `#${entry.entity_id}`,
	]);
}

export function formatAuditDetail(entry: Pick<AuditLogEntry, "presentation">) {
	return formatAuditDetailMessage(entry.presentation?.detail);
}

export function auditActionBadgeClass(action: AuditAction | string) {
	const value = String(action);
	if (value === "mail_delivery_failed") {
		return "border-red-200 bg-red-50 text-red-700 dark:border-red-900 dark:bg-red-950/60 dark:text-red-300";
	}
	if (value === "mail_send") {
		return "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900 dark:bg-emerald-950/60 dark:text-emerald-300";
	}
	if (
		value.includes("delete") ||
		value.includes("disable") ||
		value.includes("revoke") ||
		value.includes("shutdown")
	) {
		return "border-red-200 bg-red-50 text-red-700 dark:border-red-900 dark:bg-red-950/60 dark:text-red-300";
	}
	if (value.includes("login") || value.includes("test")) {
		return "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900 dark:bg-amber-950/60 dark:text-amber-300";
	}
	if (
		value.includes("create") ||
		value.includes("register") ||
		value.includes("setup")
	) {
		return "border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-900 dark:bg-sky-950/60 dark:text-sky-300";
	}
	return "border-border bg-muted/30 text-muted-foreground";
}
