import http from "k6/http";
import { check } from "k6";

const baseUrl = __ENV.ASTER_BENCH_BASE_URL || "http://127.0.0.1:3000";

export const options = {
	vus: Number(__ENV.ASTER_BENCH_RUNTIME_VUS || "4"),
	duration: __ENV.ASTER_BENCH_RUNTIME_DURATION || "10s",
	thresholds: {
		http_req_failed: ["rate<0.01"],
		http_req_duration: ["p(95)<500"],
	},
};

export default function runtimeSmoke() {
	const health = http.get(`${baseUrl}/health`);
	check(health, {
		"health is ok": (response) => response.status === 200,
	});

	const ready = http.get(`${baseUrl}/health/ready`);
	check(ready, {
		"readiness is ok": (response) => response.status === 200,
	});

	const systemInfo = http.get(`${baseUrl}/api/v1/system/info`);
	check(systemInfo, {
		"system info is ok": (response) => response.status === 200,
	});
}
