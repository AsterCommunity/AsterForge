type QueryPrimitive = string | number | boolean | null | undefined;

export function withQuery(
	path: string,
	params: Record<string, QueryPrimitive>,
) {
	const query = new URLSearchParams();
	for (const [key, value] of Object.entries(params)) {
		if (value === null || value === undefined || value === "") continue;
		query.set(key, String(value));
	}
	const rawQuery = query.toString();
	return rawQuery ? `${path}?${rawQuery}` : path;
}
