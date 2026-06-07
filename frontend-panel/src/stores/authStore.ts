import { create } from "zustand";
import { authService } from "@/services/authService";
import type { AuthUserInfo } from "@/types/api";

const USER_KEY = "asterforge-user";
const LEGACY_ACCESS_TOKEN_KEY = "asterforge-access-token";
const LEGACY_REFRESH_TOKEN_KEY = "asterforge-refresh-token";

type AuthState = {
	user: AuthUserInfo | null;
	checking: boolean;
	error: string | null;
	isAuthenticated: boolean;
	isAdmin: boolean;
	hydrate: () => Promise<void>;
	setup: (username: string, email: string, password: string) => Promise<void>;
	register: (
		username: string,
		email: string,
		password: string,
	) => Promise<void>;
	login: (identifier: string, password: string) => Promise<void>;
	refresh: () => Promise<void>;
	logout: () => Promise<void>;
	clear: () => void;
};

function readStoredUser() {
	try {
		const raw = localStorage.getItem(USER_KEY);
		return raw ? (JSON.parse(raw) as AuthUserInfo) : null;
	} catch {
		return null;
	}
}

function persistUser(user: AuthUserInfo | null) {
	if (user) {
		localStorage.setItem(USER_KEY, JSON.stringify(user));
		return;
	}
	localStorage.removeItem(USER_KEY);
}

function clearLegacyTokenStorage() {
	localStorage.removeItem(LEGACY_ACCESS_TOKEN_KEY);
	localStorage.removeItem(LEGACY_REFRESH_TOKEN_KEY);
}

function deriveAuthFlags(user: AuthUserInfo | null) {
	return {
		isAuthenticated: Boolean(user),
		isAdmin: user?.role === "admin",
	};
}

const initialUser = readStoredUser();
const initialFlags = deriveAuthFlags(initialUser);
clearLegacyTokenStorage();

export const useAuthStore = create<AuthState>((set, get) => ({
	user: initialUser,
	checking: false,
	error: null,
	isAuthenticated: initialFlags.isAuthenticated,
	isAdmin: initialFlags.isAdmin,
	async hydrate() {
		set({ checking: true, error: null });
		try {
			const user = await authService.me();
			persistUser(user);
			set({
				user,
				checking: false,
				error: null,
				...deriveAuthFlags(user),
			});
		} catch (error) {
			persistUser(null);
			set({
				user: null,
				checking: false,
				error: error instanceof Error ? error.message : "Session check failed",
				...deriveAuthFlags(null),
			});
		}
	},
	async setup(username, email, password) {
		await authService.setup({ username, email, password });
		const user = await authService.me();
		persistUser(user);
		set({ user, error: null, ...deriveAuthFlags(user) });
	},
	async register(username, email, password) {
		await authService.register({ username, email, password });
		const user = await authService.me();
		persistUser(user);
		set({ user, error: null, ...deriveAuthFlags(user) });
	},
	async login(identifier, password) {
		await authService.login({ identifier, password });
		const user = await authService.me();
		persistUser(user);
		set({ user, error: null, ...deriveAuthFlags(user) });
	},
	async refresh() {
		await authService.refresh();
		const user = await authService.me();
		persistUser(user);
		set({ user, error: null, ...deriveAuthFlags(user) });
	},
	async logout() {
		try {
			await authService.logout();
		} finally {
			get().clear();
		}
	},
	clear() {
		clearLegacyTokenStorage();
		persistUser(null);
		set({
			user: null,
			error: null,
			...deriveAuthFlags(null),
		});
	},
}));
