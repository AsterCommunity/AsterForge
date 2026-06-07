import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import App from "./App";

vi.mock("@/hooks/useServiceDiagnostics", () => ({
	useServiceDiagnostics: () => ({
		loading: false,
		updatedAt: "2026-06-07T00:00:00.000Z",
		error: null,
		refresh: vi.fn(),
		endpoints: [
			{
				id: "health",
				group: "Runtime",
				label: "Process health",
				method: "GET",
				path: "/health",
				icon: "Gauge",
				status: "ok",
				value: "ok",
				detail: "v0.1.0",
			},
			{
				id: "protected-example",
				group: "Examples",
				label: "Authenticated API",
				method: "GET",
				path: "/api/v1/examples/protected",
				icon: "Lock",
				status: "guarded",
				value: "sign-in required",
				detail: "missing session cookie",
			},
		],
	}),
}));

describe("App", () => {
	it("renders the control-panel shell and overview route", async () => {
		render(<App />);

		expect(
			await screen.findByRole("heading", { level: 1, name: "AsterForge" }),
		).toBeInTheDocument();
		expect(screen.getByRole("link", { name: "Auth" })).toBeInTheDocument();
		expect(
			screen.getAllByRole("link", { name: /API Catalog/ }).length,
		).toBeGreaterThan(0);
		expect(screen.getByText("Live service calls")).toBeInTheDocument();
		expect(screen.getAllByText("/health").length).toBeGreaterThan(0);
		expect(screen.getAllByText("sign-in required").length).toBeGreaterThan(0);
	});
});
