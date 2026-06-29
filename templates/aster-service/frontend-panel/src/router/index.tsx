import { createBrowserRouter } from "react-router-dom";
import { AppShell } from "@/components/layout/AppShell";
import { OperationsPage } from "@/pages/OperationsPage";
import { OverviewPage } from "@/pages/OverviewPage";
import { SettingsPage } from "@/pages/SettingsPage";

export const router = createBrowserRouter([
	{
		path: "/",
		element: <AppShell />,
		children: [
			{ index: true, element: <OverviewPage /> },
			{ path: "operations", element: <OperationsPage /> },
			{ path: "settings", element: <SettingsPage /> },
		],
	},
]);
