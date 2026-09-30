type JsonRequestInit = Omit<RequestInit, "headers"> & {
	headers?: HeadersInit;
};

export class HttpError extends Error {
	readonly status: number;
	readonly payload: unknown;

	constructor(status: number, payload: unknown) {
		super(`HTTP request failed with status ${status}`);
		this.name = "HttpError";
		this.status = status;
		this.payload = payload;
	}
}

export async function requestJson<TResponse>(
	input: RequestInfo | URL,
	init: JsonRequestInit = {},
) {
	const response = await fetch(input, {
		...init,
		headers: {
			Accept: "application/json",
			...init.headers,
		},
	});
	const text = await response.text();
	const payload = text ? JSON.parse(text) : null;

	if (!response.ok) {
		throw new HttpError(response.status, payload);
	}

	return payload as TResponse;
}
