import type { components } from "./api.generated";

type DriveProfile =
	components["schemas"]["ApiResponse_Profile_DriveCode_Diagnostic"];
type DriveTask = components["schemas"]["ApiResponse_Task_DriveCode_Diagnostic"];
type GateProfile =
	components["schemas"]["ApiResponse_Profile_GateCode_TupleUnit"];
type GateGroup = components["schemas"]["ApiResponse_Group_GateCode_TupleUnit"];
type DriveFailure =
	components["schemas"]["ApiResponse_TupleUnit_DriveCode_Diagnostic"];

const profile: DriveProfile = {
	code: "success",
	msg: "",
	data: { id: 7, name: "cat" },
};
const task: DriveTask = {
	code: "success",
	msg: "",
	data: { id: 1, completed: true },
};
const gate: GateProfile = {
	code: "account.identifier_conflict",
	msg: "conflict",
	error: { retryable: false },
};
const group: GateGroup = {
	code: "success",
	msg: "",
	data: { owner: { id: 7, name: "cat" } },
};
const failure: DriveFailure = {
	code: "rate_limited",
	msg: "wait",
	error: {
		retryable: true,
		diagnostic: { kind: "transient", message: "retry later" },
	},
};
const nullData: DriveFailure = { code: "success", msg: "", data: null };
void [profile, task, gate, group, failure, nullData];

// @ts-expect-error Product codes must not leak across envelope schemas.
const wrongCode: DriveProfile = {
	code: "account.identifier_conflict",
	msg: "",
};
// @ts-expect-error Task fields must not replace a profile DTO.
const wrongData: DriveProfile = {
	code: "success",
	msg: "",
	data: { id: 1, completed: true },
};
// @ts-expect-error Error metadata retains a boolean retryability contract.
const wrongMetadata: DriveFailure = {
	code: "rate_limited",
	msg: "",
	error: { retryable: "yes" },
};
// @ts-expect-error Typed diagnostics reject unvalidated field types.
const wrongDiagnostic: DriveFailure = {
	code: "rate_limited",
	msg: "",
	error: { retryable: true, diagnostic: { kind: 42, message: "bad" } },
};
// @ts-expect-error Unit-valued data is JSON null, not unconstrained JSON.
const wrongUnit: DriveFailure = { code: "success", msg: "", data: {} };
// @ts-expect-error Nested DTO references retain their required fields.
const wrongGroup: GateGroup = {
	code: "success",
	msg: "",
	data: { owner: { id: 7 } },
};
// @ts-expect-error A missing diagnostic type cannot carry arbitrary diagnostic JSON.
const wrongGateDiagnostic: GateProfile = {
	code: "account.identifier_conflict",
	msg: "",
	error: { retryable: false, diagnostic: {} },
};
void [
	wrongCode,
	wrongData,
	wrongMetadata,
	wrongDiagnostic,
	wrongUnit,
	wrongGroup,
	wrongGateDiagnostic,
];
