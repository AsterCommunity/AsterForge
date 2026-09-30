import { createBrowserRouter } from "react-router-dom";
import { AppShell } from "@/components/layout/AppShell";
import { ErrorPage } from "@/pages/ErrorPage";
import { OperationsPage } from "@/pages/OperationsPage";
import { OverviewPage } from "@/pages/OverviewPage";
import { SettingsPage } from "@/pages/SettingsPage";
import { appPaths } from "@/routes/routePaths";

export const router = createBrowserRouter([
	{
		path: appPaths.overview,
		element: <AppShell />,
		errorElement: <ErrorPage />,
		children: [
			{ index: true, element: <OverviewPage /> },
			{ path: appPaths.operations.slice(1), element: <OperationsPage /> },
			{ path: appPaths.settings.slice(1), element: <SettingsPage /> },
			{ path: "*", element: <ErrorPage /> },
		],
	},
]);
