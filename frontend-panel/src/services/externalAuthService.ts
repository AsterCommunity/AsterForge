import { withQuery } from "@/lib/query";
import type {
	ExternalAuthFinishLoginResponse,
	ExternalAuthKind,
	ExternalAuthPublicProvider,
	ExternalAuthStartLoginRequest,
	ExternalAuthStartLoginResponse,
} from "@/types/api";
import { api } from "./http";

export const externalAuthService = {
	listPublic: (signal?: AbortSignal) =>
		api.get<ExternalAuthPublicProvider[]>("/external-auth/providers", {
			signal,
		}),
	startPublic: (provider: string, data: ExternalAuthStartLoginRequest) =>
		api.post<ExternalAuthStartLoginResponse>(
			`/external-auth/${encodeURIComponent(provider)}/start`,
			data,
		),
	finishPublic: (provider: string, state: string, code: string) =>
		api.get<ExternalAuthFinishLoginResponse>(
			withQuery(`/external-auth/${encodeURIComponent(provider)}/callback`, {
				state,
				code,
			}),
		),
	listAuthAliases: (signal?: AbortSignal) =>
		api.get<ExternalAuthPublicProvider[]>("/auth/external-auth/providers", {
			signal,
		}),
	listAuthAliasesByKind: (kind: ExternalAuthKind, signal?: AbortSignal) =>
		api.get<ExternalAuthPublicProvider[]>(
			`/auth/external-auth/${encodeURIComponent(kind)}/providers`,
			{ signal },
		),
	startAuthAlias: (
		kind: ExternalAuthKind,
		provider: string,
		data: ExternalAuthStartLoginRequest,
	) =>
		api.post<ExternalAuthStartLoginResponse>(
			`/auth/external-auth/${encodeURIComponent(kind)}/${encodeURIComponent(
				provider,
			)}/start`,
			data,
		),
	finishAuthAlias: (
		kind: ExternalAuthKind,
		provider: string,
		state: string,
		code: string,
	) =>
		api.get<ExternalAuthFinishLoginResponse>(
			withQuery(
				`/auth/external-auth/${encodeURIComponent(kind)}/${encodeURIComponent(
					provider,
				)}/callback`,
				{ state, code },
			),
		),
};
