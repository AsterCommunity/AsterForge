export function EndpointCode({ children }: { children: string }) {
	return (
		<code className="inline-flex w-fit rounded-md border border-blue-200 bg-blue-50 px-2 py-1 font-bold font-mono text-blue-700 text-xs dark:border-blue-400/30 dark:bg-blue-400/10 dark:text-blue-200">
			{children}
		</code>
	);
}
