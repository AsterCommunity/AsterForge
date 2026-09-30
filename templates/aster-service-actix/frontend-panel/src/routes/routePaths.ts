export const appPaths = {
	overview: "/",
	operations: "/operations",
	settings: "/settings",
} as const;

export const backendPaths = {
	api: "/api/v1",
	health: "/health",
	readiness: "/health/ready",
	metrics: "/health/metrics",
	openapi: "/api-docs/openapi.json",
	swaggerUi: "/swagger-ui/",
} as const;

export const backendPathDenylist = [
	/^\/api(?:\/.*)?$/,
	/^\/health(?:\/.*)?$/,
	/^\/api-docs(?:\/.*)?$/,
	/^\/swagger-ui(?:\/.*)?$/,
] as const;
