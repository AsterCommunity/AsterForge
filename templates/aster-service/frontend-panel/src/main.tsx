import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "react-router-dom";
import "./index.css";
import { router } from "@/router/index";

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

root.querySelector("[data-aster-boot-loading]")?.remove();
createRoot(root).render(
	<StrictMode>
		<RouterProvider router={router} />
	</StrictMode>,
);
