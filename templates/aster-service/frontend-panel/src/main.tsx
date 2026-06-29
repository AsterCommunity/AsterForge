import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "@/app/App";
import { AppProviders } from "@/app/AppProviders";
import "@/i18n";
import "./index.css";

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

root.querySelector("[data-aster-boot-loading]")?.remove();
createRoot(root).render(
	<StrictMode>
		<AppProviders>
			<App />
		</AppProviders>
	</StrictMode>,
);
