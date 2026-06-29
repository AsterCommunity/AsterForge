import { expect, test } from "playwright/test";

test("serves the app shell and frontend routes", async ({ page }) => {
	await page.addInitScript(() => {
		window.localStorage.setItem("aster-service.language", "en-US");
	});
	await page.goto("/");
	await expect(
		page.getByRole("heading", { name: /service core is ready/i }),
	).toBeVisible();

	await page.getByRole("link", { name: "Operations" }).click();
	await expect(page).toHaveURL(/\/operations$/);
	await expect(
		page.getByRole("heading", { name: /operational surfaces/i }),
	).toBeVisible();

	await page.goto("/missing-route");
	await expect(
		page.getByRole("heading", { name: "Page not found" }),
	).toBeVisible();
});
