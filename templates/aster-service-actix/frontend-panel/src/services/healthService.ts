import { backendPaths } from "@/routes/routePaths";
import { requestJson } from "@/services/httpClient";
import type { StatusResponse } from "@/types/api";

export function getHealth() {
	return requestJson<StatusResponse>(backendPaths.health);
}

export function getReadiness() {
	return requestJson<StatusResponse>(backendPaths.readiness);
}
