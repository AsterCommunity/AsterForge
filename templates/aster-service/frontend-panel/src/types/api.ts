import type {
	operations as ApiOperations,
	components,
} from "@/types/api.generated";

export type { operations, paths } from "@/types/api.generated";

type OperationResponses<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends { responses: infer Responses }
		? Responses
		: never;

type OperationJsonContent<
	Operation extends keyof ApiOperations,
	Status extends
		keyof OperationResponses<Operation> = 200 extends keyof OperationResponses<Operation>
		? 200
		: keyof OperationResponses<Operation>,
> = OperationResponses<Operation>[Status] extends {
	content: {
		"application/json": infer Body;
	};
}
	? NonNullable<Body>
	: never;

export type OperationJsonResponse<
	Operation extends keyof ApiOperations,
	Status extends
		keyof OperationResponses<Operation> = 200 extends keyof OperationResponses<Operation>
		? 200
		: keyof OperationResponses<Operation>,
> = OperationJsonContent<Operation, Status>;

export type OperationData<
	Operation extends keyof ApiOperations,
	Status extends
		keyof OperationResponses<Operation> = 200 extends keyof OperationResponses<Operation>
		? 200
		: keyof OperationResponses<Operation>,
> =
	OperationJsonResponse<Operation, Status> extends { data?: infer Data }
		? NonNullable<Data>
		: never;

export type OperationQuery<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends { parameters: { query?: infer Query } }
		? NonNullable<Query>
		: never;

export type OperationPath<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends { parameters: { path: infer Path } }
		? NonNullable<Path>
		: never;

export type OperationRequestBody<Operation extends keyof ApiOperations> =
	ApiOperations[Operation] extends {
		requestBody: {
			content: {
				"application/json": infer Body;
			};
		};
	}
		? NonNullable<Body>
		: never;

export type StatusResponse = components["schemas"] extends {
	StatusResponse: infer Schema;
}
	? Schema
	: {
			service: string;
			status: string;
		};

export type ErrorResponse = components["schemas"] extends {
	ErrorResponse: infer Schema;
}
	? Schema
	: {
			service: string;
			code: string;
			message: string;
		};
