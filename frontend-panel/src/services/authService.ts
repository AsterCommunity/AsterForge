import type {
	AuthSessionInfo,
	AuthTokenResponse,
	AuthUserInfo,
	CheckResp,
	LoginRequest,
	RegisterRequest,
	SetupRequest,
} from "@/types/api";
import { api } from "./http";

export const authService = {
	check: () => api.get<CheckResp>("/auth/check"),
	setup: (data: SetupRequest) =>
		api.post<AuthTokenResponse>("/auth/setup", data),
	register: (data: RegisterRequest) =>
		api.post<AuthTokenResponse>("/auth/register", data),
	login: (data: LoginRequest) =>
		api.post<AuthTokenResponse>("/auth/login", data),
	refresh: () => api.post<AuthTokenResponse>("/auth/refresh"),
	logout: () => api.post<void>("/auth/logout"),
	me: () => api.get<AuthUserInfo>("/auth/me"),
	sessions: () => api.get<AuthSessionInfo[]>("/auth/sessions"),
};
