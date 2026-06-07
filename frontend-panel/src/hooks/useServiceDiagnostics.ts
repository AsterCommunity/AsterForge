import { useCallback, useEffect, useRef, useState } from "react";
import {
	createIdleDiagnostics,
	loadServiceDiagnostics,
	type ServiceDiagnosticResult,
} from "@/services/diagnosticsService";
import { formatUnknownError } from "@/services/http";

export type ServiceDiagnosticsSnapshot = {
	loading: boolean;
	updatedAt: string | null;
	error: string | null;
	endpoints: ServiceDiagnosticResult[];
};

export type ServiceDiagnosticsState = ServiceDiagnosticsSnapshot & {
	refresh: () => void;
};

const initialSnapshot: ServiceDiagnosticsSnapshot = {
	loading: false,
	updatedAt: null,
	error: null,
	endpoints: createIdleDiagnostics(),
};

export function useServiceDiagnostics(): ServiceDiagnosticsState {
	const abortRef = useRef<AbortController | null>(null);
	const [snapshot, setSnapshot] =
		useState<ServiceDiagnosticsSnapshot>(initialSnapshot);

	const refresh = useCallback(() => {
		abortRef.current?.abort();
		const controller = new AbortController();
		abortRef.current = controller;

		setSnapshot((current) => ({
			...current,
			loading: true,
			error: null,
			endpoints: createIdleDiagnostics("loading"),
		}));

		void loadServiceDiagnostics(controller.signal)
			.then((endpoints) => {
				if (controller.signal.aborted) return;
				setSnapshot({
					loading: false,
					updatedAt: new Date().toISOString(),
					error: null,
					endpoints,
				});
			})
			.catch((error: unknown) => {
				if (controller.signal.aborted) return;
				setSnapshot((current) => ({
					...current,
					loading: false,
					updatedAt: new Date().toISOString(),
					error: formatUnknownError(error),
				}));
			});
	}, []);

	useEffect(() => {
		refresh();
		return () => {
			abortRef.current?.abort();
		};
	}, [refresh]);

	return {
		...snapshot,
		refresh,
	};
}
