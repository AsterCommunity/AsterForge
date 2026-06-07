import { expect, test } from "@playwright/test";

test("serves the Forge control panel and core navigation", async ({ page }) => {
	await page.goto("/");
	await expect(page).toHaveURL(/\/overview$/);
	await expect(page.getByRole("heading", { name: "AsterForge" })).toBeVisible();
	await expect(page.getByText("Registered APIs")).toBeVisible();

	await page.getByRole("link", { name: "API Catalog" }).first().click();
	await expect(page).toHaveURL(/\/api-catalog$/);
	await expect(
		page.getByRole("heading", { name: "API Catalog" }),
	).toBeVisible();
	await expect(page.getByLabel("Filter API catalog")).toBeVisible();

	await page.getByRole("link", { name: "Auth", exact: true }).click();
	await expect(page).toHaveURL(/\/auth$/);
	await expect(
		page.getByRole("heading", { name: "Auth", exact: true }),
	).toBeVisible();
	await expect(page.getByLabel("Mode")).toBeVisible();
	await expect(page.getByLabel("Identifier")).toBeVisible();
	await expect(page.getByLabel("Password")).toBeVisible();
});

test("supports the mobile navigation drawer", async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await page.goto("/overview");

	await page.getByRole("button", { name: "Open navigation" }).click();
	await expect(
		page.getByRole("button", { name: "Close navigation" }),
	).toBeVisible();

	await page.getByRole("link", { name: "Tasks" }).click();
	await expect(page).toHaveURL(/\/admin\/tasks$/);
	await expect(
		page.getByRole("heading", { name: "Admin Tasks" }),
	).toBeVisible();
});
