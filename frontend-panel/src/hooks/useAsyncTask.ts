import { useCallback, useState } from "react";
import { formatUnknownError } from "@/services/http";

export type AsyncTaskState<T> = {
	loading: boolean;
	error: string | null;
	result: T | null;
	updatedAt: string | null;
};

const initialState = {
	loading: false,
	error: null,
	result: null,
	updatedAt: null,
};

export function useAsyncTask<T>() {
	const [state, setState] = useState<AsyncTaskState<T>>(initialState);

	const run = useCallback(async (task: () => Promise<T>) => {
		setState((current) => ({ ...current, loading: true, error: null }));
		try {
			const result = await task();
			setState({
				loading: false,
				error: null,
				result,
				updatedAt: new Date().toISOString(),
			});
			return result;
		} catch (error) {
			setState((current) => ({
				...current,
				loading: false,
				error: formatUnknownError(error),
				updatedAt: new Date().toISOString(),
			}));
			return undefined;
		}
	}, []);

	const reset = useCallback(() => {
		setState(initialState);
	}, []);

	return { ...state, run, reset };
}
