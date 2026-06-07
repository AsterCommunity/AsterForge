export function emptyToNull(value: string) {
	const trimmed = value.trim();
	return trimmed ? trimmed : null;
}

export function emptyToUndefined(value: string) {
	const trimmed = value.trim();
	return trimmed ? trimmed : undefined;
}

export function numberOrUndefined(value: string) {
	if (!value.trim()) return undefined;
	const parsed = Number(value);
	return Number.isFinite(parsed) ? parsed : undefined;
}

export function integerOrUndefined(value: string) {
	const parsed = numberOrUndefined(value);
	return parsed === undefined ? undefined : Math.trunc(parsed);
}

export function dateTimeLocalToIso(value: string) {
	if (!value.trim()) return undefined;
	const date = new Date(value);
	return Number.isNaN(date.getTime()) ? undefined : date.toISOString();
}

export function parseStringOrStringArray(value: string) {
	const trimmed = value.trim();
	if (!trimmed) return "";

	try {
		const parsed = JSON.parse(trimmed) as unknown;
		if (typeof parsed === "string") return parsed;
		if (Array.isArray(parsed)) {
			return parsed.map((item) => String(item));
		}
	} catch {
		// Fall through to newline parsing.
	}

	if (trimmed.includes("\n")) {
		return trimmed
			.split("\n")
			.map((line) => line.trim())
			.filter(Boolean);
	}

	return trimmed;
}

export function compactRecord<T extends Record<string, unknown>>(value: T) {
	return Object.fromEntries(
		Object.entries(value).filter(([, entry]) => entry !== undefined),
	) as T;
}
