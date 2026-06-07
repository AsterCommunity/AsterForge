import { useCallback, useEffect, useState } from "react";
import { Link, NavLink, Outlet } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { buttonVariants } from "@/components/ui/buttonVariants";
import { Icon, type IconName } from "@/components/ui/icon";
import { Separator } from "@/components/ui/separator";
import { cn, sidebarNavItemClass } from "@/lib/utils";
import { useAuthStore } from "@/stores/authStore";

type NavItem = {
	to: string;
	label: string;
	icon: IconName;
	end?: boolean;
};

const primaryNav: NavItem[] = [
	{ to: "/overview", label: "Overview", icon: "Gauge" },
	{ to: "/auth", label: "Auth", icon: "Key" },
	{ to: "/external-auth", label: "External Auth", icon: "Globe" },
	{ to: "/admin/config", label: "Config", icon: "Gear" },
	{ to: "/admin/external-auth", label: "Admin SSO", icon: "SignIn" },
	{ to: "/admin/tasks", label: "Tasks", icon: "Clock" },
	{ to: "/admin/audit", label: "Audit", icon: "ClipboardText" },
	{ to: "/api-catalog", label: "API Catalog", icon: "Table" },
];

export function AppLayout() {
	const [mobileOpen, setMobileOpen] = useState(false);
	const user = useAuthStore((state) => state.user);
	const checking = useAuthStore((state) => state.checking);
	const isAdmin = useAuthStore((state) => state.isAdmin);
	const isAuthenticated = useAuthStore((state) => state.isAuthenticated);
	const hydrate = useAuthStore((state) => state.hydrate);
	const logout = useAuthStore((state) => state.logout);

	useEffect(() => {
		void hydrate();
	}, [hydrate]);

	const closeMobile = useCallback(() => setMobileOpen(false), []);

	const sidebar = (
		<div className="flex h-full flex-col bg-sidebar text-sidebar-foreground">
			<Link
				to="/overview"
				onClick={closeMobile}
				className="flex h-14 items-center gap-2 border-b border-sidebar-border px-4"
			>
				<span className="flex size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground">
					<Icon name="Wrench" className="size-4" />
				</span>
				<span className="min-w-0">
					<span className="block truncate text-sm font-semibold">
						AsterForge
					</span>
					<span className="block truncate text-xs text-muted-foreground">
						Control panel
					</span>
				</span>
			</Link>
			<nav className="flex-1 space-y-1 overflow-y-auto px-2 py-3">
				{primaryNav.map((item) => (
					<NavLink
						key={item.to}
						to={item.to}
						end={item.end}
						onClick={closeMobile}
						className={({ isActive }) => sidebarNavItemClass(isActive)}
					>
						<Icon name={item.icon} className="size-4 shrink-0" />
						<span className="truncate">{item.label}</span>
					</NavLink>
				))}
			</nav>
			<div className="border-t border-sidebar-border p-3">
				<div className="rounded-lg border border-sidebar-border bg-background/55 p-2">
					<div className="flex items-center gap-2">
						<span
							className={cn(
								"size-2 rounded-full",
								isAuthenticated ? "bg-emerald-600" : "bg-muted-foreground",
							)}
						/>
						<span className="truncate text-xs font-medium">
							{user?.username ?? "No session"}
						</span>
					</div>
					<div className="mt-1 text-xs text-muted-foreground">
						{checking
							? "checking"
							: isAdmin
								? "admin"
								: (user?.role ?? "public")}
					</div>
				</div>
			</div>
		</div>
	);

	return (
		<div className="flex h-dvh overflow-hidden bg-background text-foreground">
			<button
				type="button"
				aria-label="Close navigation"
				className={cn(
					"fixed inset-0 z-30 bg-black/35 transition-opacity md:hidden",
					mobileOpen ? "opacity-100" : "pointer-events-none opacity-0",
				)}
				onClick={closeMobile}
			/>
			<aside
				className={cn(
					"fixed inset-y-0 left-0 z-40 w-64 border-r border-sidebar-border transition-transform md:relative md:translate-x-0",
					mobileOpen ? "translate-x-0" : "-translate-x-full",
				)}
			>
				{sidebar}
			</aside>
			<div className="flex min-w-0 flex-1 flex-col">
				<header className="flex h-14 shrink-0 items-center gap-2 border-b border-border/70 bg-background/95 px-3 backdrop-blur sm:px-4">
					<Button
						type="button"
						size="icon"
						variant="ghost"
						className="md:hidden"
						onClick={() => setMobileOpen((open) => !open)}
						aria-label="Open navigation"
					>
						<Icon name="List" className="size-4" />
					</Button>
					<div className="min-w-0 flex-1">
						<div className="truncate text-sm font-medium">
							Operational API console
						</div>
						<div className="truncate text-xs text-muted-foreground">
							{isAuthenticated
								? `${user?.email ?? "authenticated"}`
								: "public mode"}
						</div>
					</div>
					<Separator orientation="vertical" className="hidden h-6 sm:block" />
					{isAuthenticated ? (
						<Button
							type="button"
							variant="outline"
							size="sm"
							onClick={() => void logout()}
						>
							<Icon name="SignOut" className="size-4" />
							Logout
						</Button>
					) : (
						<Link
							to="/auth"
							className={cn(buttonVariants({ variant: "outline", size: "sm" }))}
						>
							<Icon name="SignIn" className="size-4" />
							Login
						</Link>
					)}
				</header>
				<main className="min-h-0 flex-1 overflow-y-auto bg-[linear-gradient(90deg,oklch(0.895_0.006_255_/_0.28)_1px,transparent_1px),linear-gradient(180deg,oklch(0.895_0.006_255_/_0.22)_1px,transparent_1px)] bg-[length:36px_36px]">
					<Outlet />
				</main>
			</div>
		</div>
	);
}
