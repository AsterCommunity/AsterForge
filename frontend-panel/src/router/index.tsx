import { createBrowserRouter, Navigate } from "react-router-dom";
import { AppLayout } from "@/components/layout/AppLayout";
import ApiCatalogPage from "@/pages/ApiCatalogPage";
import AuthPage from "@/pages/AuthPage";
import AdminAuditPage from "@/pages/admin/AdminAuditPage";
import AdminConfigPage from "@/pages/admin/AdminConfigPage";
import AdminExternalAuthPage from "@/pages/admin/AdminExternalAuthPage";
import AdminTasksPage from "@/pages/admin/AdminTasksPage";
import ErrorPage from "@/pages/ErrorPage";
import ExternalAuthPage from "@/pages/ExternalAuthPage";
import OverviewPage from "@/pages/OverviewPage";

export const router = createBrowserRouter([
	{
		element: <AppLayout />,
		errorElement: <ErrorPage />,
		children: [
			{ index: true, element: <Navigate to="/overview" replace /> },
			{ path: "/overview", element: <OverviewPage /> },
			{ path: "/auth", element: <AuthPage /> },
			{ path: "/external-auth", element: <ExternalAuthPage /> },
			{ path: "/admin/config", element: <AdminConfigPage /> },
			{ path: "/admin/external-auth", element: <AdminExternalAuthPage /> },
			{ path: "/admin/tasks", element: <AdminTasksPage /> },
			{ path: "/admin/audit", element: <AdminAuditPage /> },
			{ path: "/api-catalog", element: <ApiCatalogPage /> },
		],
	},
	{ path: "*", element: <Navigate to="/overview" replace /> },
]);
