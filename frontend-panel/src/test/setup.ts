import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

class ResizeObserverMock {
	observe() {}
	unobserve() {}
	disconnect() {}
}

class IntersectionObserverMock {
	root = null;
	rootMargin = "";
	thresholds = [];

	disconnect() {}
	observe() {}
	takeRecords() {
		return [];
	}
	unobserve() {}
}

Object.defineProperty(window, "matchMedia", {
	writable: true,
	value: vi.fn().mockImplementation((query: string) => ({
		matches: false,
		media: query,
		onchange: null,
		addEventListener: vi.fn(),
		removeEventListener: vi.fn(),
		addListener: vi.fn(),
		removeListener: vi.fn(),
		dispatchEvent: vi.fn(),
	})),
});

Object.defineProperty(window, "ResizeObserver", {
	writable: true,
	value: ResizeObserverMock,
});

Object.defineProperty(window, "IntersectionObserver", {
	writable: true,
	value: IntersectionObserverMock,
});

Object.defineProperty(window, "scrollTo", {
	writable: true,
	value: vi.fn(),
});

Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
	writable: true,
	value: vi.fn(),
});

Object.defineProperty(Element.prototype, "getAnimations", {
	writable: true,
	value: vi.fn(() => []),
});

afterEach(() => {
	cleanup();
	localStorage.clear();
	sessionStorage.clear();
	document.documentElement.className = "";
	document.documentElement.removeAttribute("data-theme");
	vi.useRealTimers();
});
