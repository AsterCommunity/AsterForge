import type { ReactNode } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

export function Field({
	label,
	htmlFor,
	children,
	className,
}: {
	label: string;
	htmlFor?: string;
	children: ReactNode;
	className?: string;
}) {
	return (
		<div className={cn("grid gap-1.5", className)}>
			<Label htmlFor={htmlFor}>{label}</Label>
			{children}
		</div>
	);
}

export function TextField({
	label,
	value,
	onChange,
	type = "text",
	placeholder,
	required,
	className,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	type?: string;
	placeholder?: string;
	required?: boolean;
	className?: string;
}) {
	const id = label.toLowerCase().replaceAll(/\W+/g, "-");
	return (
		<Field label={label} htmlFor={id} className={className}>
			<Input
				id={id}
				type={type}
				value={value}
				placeholder={placeholder}
				required={required}
				onChange={(event) => onChange(event.currentTarget.value)}
			/>
		</Field>
	);
}

export function TextareaField({
	label,
	value,
	onChange,
	placeholder,
	className,
	rows,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	placeholder?: string;
	className?: string;
	rows?: number;
}) {
	const id = label.toLowerCase().replaceAll(/\W+/g, "-");
	return (
		<Field label={label} htmlFor={id} className={className}>
			<Textarea
				id={id}
				value={value}
				placeholder={placeholder}
				rows={rows}
				onChange={(event) => onChange(event.currentTarget.value)}
			/>
		</Field>
	);
}

export function NativeSelectField({
	label,
	value,
	onChange,
	options,
	className,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	options: { label: string; value: string }[];
	className?: string;
}) {
	const id = label.toLowerCase().replaceAll(/\W+/g, "-");
	return (
		<Field label={label} htmlFor={id} className={className}>
			<select
				id={id}
				value={value}
				onChange={(event) => onChange(event.currentTarget.value)}
				className="h-8 w-full rounded-lg border border-input/80 bg-card/70 px-2.5 text-sm shadow-xs outline-none transition-[background-color,border-color,box-shadow] focus-visible:border-ring focus-visible:bg-background focus-visible:ring-3 focus-visible:ring-ring/30 dark:bg-input/25 dark:shadow-none"
			>
				{options.map((option) => (
					<option key={option.value} value={option.value}>
						{option.label}
					</option>
				))}
			</select>
		</Field>
	);
}
