import type {
	AxiosInstance,
	AxiosRequestConfig,
	AxiosResponse,
	InternalAxiosRequestConfig,
} from "axios";
import axios, { AxiosHeaders } from "axios";
import { config } from "@/config/app";
import type { ApiErrorInfo, ApiResponse, AsterErrorCode } from "@/types/api";
import { CSRF_HEADER_NAME, getCsrfToken } from "./csrf";

const client: AxiosInstance = axios.create({
	baseURL: config.apiBaseUrl,
	timeout: 15_000,
	headers: { "Content-Type": "application/json" },
	withCredentials: true,
});

const rootClient: AxiosInstance = axios.create({
	baseURL: config.rootBaseUrl,
	timeout: 15_000,
	headers: { "Content-Type": "application/json" },
	withCredentials: true,
});

export type ApiRequestConfig = Pick<
	AxiosRequestConfig,
	"data" | "headers" | "params" | "signal"
>;

type RetriableRequestConfig = InternalAxiosRequestConfig & {
	_retry?: boolean;
};

export class ApiError extends Error {
	code: AsterErrorCode;
	retryable?: boolean;

	constructor(
		code: AsterErrorCode,
		message: string,
		details: Pick<ApiErrorInfo, "retryable"> = {},
	) {
		super(message);
		this.code = code;
		this.retryable = details.retryable ?? undefined;
	}
}

function isApiEnvelope(value: unknown): value is ApiResponse<unknown> {
	if (typeof value !== "object" || value === null) return false;
	return "code" in value && "msg" in value;
}

function normalizeApiErrorInfo(
	value: unknown,
): Pick<ApiErrorInfo, "retryable"> {
	if (typeof value !== "object" || value === null) return {};
	const retryable = "retryable" in value ? value.retryable : undefined;
	return typeof retryable === "boolean" ? { retryable } : {};
}

function extractApiError(error: unknown): ApiError | null {
	if (error instanceof ApiError) return error;
	if (typeof error !== "object" || error === null) return null;

	const response =
		"response" in error && typeof error.response === "object"
			? error.response
			: null;
	const data = response && "data" in response ? response.data : null;
	if (!isApiEnvelope(data)) return null;
	if (typeof data.code !== "string" || typeof data.msg !== "string") {
		return null;
	}

	return new ApiError(
		data.code as AsterErrorCode,
		data.msg || "API request failed",
		normalizeApiErrorInfo(data.error),
	);
}

function isTokenAuthError(error: unknown) {
	return (
		error instanceof ApiError &&
		(error.code === "auth.token_invalid" || error.code === "auth.token_expired")
	);
}

function isRequestCanceled(error: unknown): boolean {
	if (typeof axios.isCancel === "function" && axios.isCancel(error)) {
		return true;
	}
	if (typeof error !== "object" || error === null) {
		return false;
	}
	const code = "code" in error ? error.code : null;
	const name = "name" in error ? error.name : null;
	return code === "ERR_CANCELED" || name === "AbortError";
}

const SKIP_REFRESH_PATHS = [
	"/auth/refresh",
	"/auth/login",
	"/auth/register",
	"/auth/logout",
	"/auth/check",
	"/auth/setup",
	"/auth/external-auth/providers",
];

function shouldSkipRefresh(url: string) {
	return SKIP_REFRESH_PATHS.some((path) => url.endsWith(path));
}

function isUnsafeMethod(method?: string) {
	return !["get", "head", "options", "trace"].includes(
		(method ?? "get").toLowerCase(),
	);
}

function hasHeader(
	headers: InternalAxiosRequestConfig["headers"],
	name: string,
): boolean {
	if (!headers) return false;
	if ("get" in headers && typeof headers.get === "function") {
		return headers.get(name) != null;
	}
	return Object.keys(headers).some(
		(key) => key.toLowerCase() === name.toLowerCase(),
	);
}

function setHeader(
	request: InternalAxiosRequestConfig,
	name: string,
	value: string,
) {
	if (
		request.headers &&
		"set" in request.headers &&
		typeof request.headers.set === "function"
	) {
		request.headers.set(name, value);
		return;
	}

	request.headers = AxiosHeaders.from(request.headers ?? {});
	request.headers.set(name, value);
}

export function formatUnknownError(error: unknown): string {
	if (error instanceof Error && error.message) return error.message;
	if (typeof error === "string" && error.trim()) return error;
	return "Request failed";
}

async function unwrap<T>(
	promise: Promise<AxiosResponse<ApiResponse<T>>>,
): Promise<T> {
	try {
		const { data: response } = await promise;
		if (response === undefined || response === null) {
			return undefined as T;
		}
		if (!isApiEnvelope(response)) {
			throw new ApiError(
				"internal_server_error",
				"Invalid API response envelope",
			);
		}
		if (response.code !== "success") {
			throw new ApiError(
				response.code,
				response.msg || "API request failed",
				normalizeApiErrorInfo(response.error),
			);
		}
		return response.data as T;
	} catch (error) {
		throw extractApiError(error) ?? error;
	}
}

function createApi(axiosClient: AxiosInstance) {
	return {
		get: <T>(url: string, requestConfig?: ApiRequestConfig) =>
			unwrap<T>(axiosClient.get<ApiResponse<T>>(url, requestConfig)),
		post: <T>(url: string, data?: unknown, requestConfig?: ApiRequestConfig) =>
			unwrap<T>(axiosClient.post<ApiResponse<T>>(url, data, requestConfig)),
		put: <T>(url: string, data?: unknown, requestConfig?: ApiRequestConfig) =>
			unwrap<T>(axiosClient.put<ApiResponse<T>>(url, data, requestConfig)),
		patch: <T>(url: string, data?: unknown, requestConfig?: ApiRequestConfig) =>
			unwrap<T>(axiosClient.patch<ApiResponse<T>>(url, data, requestConfig)),
		delete: <T>(url: string, requestConfig?: ApiRequestConfig) =>
			unwrap<T>(axiosClient.delete<ApiResponse<T>>(url, requestConfig)),
		client: axiosClient,
	};
}

function attachCsrfHeader(request: InternalAxiosRequestConfig) {
	const csrfToken = getCsrfToken();
	if (!csrfToken || !isUnsafeMethod(request.method)) return request;
	if (!hasHeader(request.headers, CSRF_HEADER_NAME)) {
		setHeader(request, CSRF_HEADER_NAME, csrfToken);
	}
	return request;
}

client.interceptors.request.use(attachCsrfHeader);
rootClient.interceptors.request.use(attachCsrfHeader);

let isRefreshing = false;
let refreshPromise: Promise<void> | null = null;

client.interceptors.response.use(
	(response) => response,
	async (error) => {
		if (isRequestCanceled(error)) {
			return Promise.reject(error);
		}

		const original = error.config as RetriableRequestConfig | undefined;
		const url = original?.url ?? "";
		const apiError = extractApiError(error);
		if (
			error.response?.status === 401 &&
			original &&
			!original._retry &&
			!shouldSkipRefresh(url) &&
			isTokenAuthError(apiError)
		) {
			original._retry = true;
			if (!isRefreshing) {
				isRefreshing = true;
				refreshPromise = (async () => {
					const { useAuthStore } = await import("@/stores/authStore");
					await useAuthStore.getState().refresh();
				})().finally(() => {
					isRefreshing = false;
					refreshPromise = null;
				});
			}
			try {
				await refreshPromise;
				return client(original);
			} catch (refreshError) {
				const { useAuthStore } = await import("@/stores/authStore");
				useAuthStore.getState().clear();
				return Promise.reject(extractApiError(refreshError) ?? refreshError);
			}
		}

		return Promise.reject(apiError ?? error);
	},
);

const versionedApi = createApi(client);

export const api = {
	...versionedApi,
	root: createApi(rootClient),
	client,
	rootClient,
};
