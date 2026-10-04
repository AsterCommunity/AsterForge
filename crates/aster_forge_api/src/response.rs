//! Typed REST envelopes. Products own codes, messages, HTTP behavior and diagnostic policy.
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

#[cfg(all(debug_assertions, feature = "openapi"))]
mod schema;

/// Product-provided classification of a serialized code. Forge supplies no business codes.
pub trait ApiResponseCode {
    /// Whether this code denotes success; all remaining codes denote failures.
    fn is_success(&self) -> bool;
}

/// Invalid envelope state, independent of product-facing errors and HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResponseEnvelopeError {
    /// A success constructor received a failure code.
    #[error("success response requires a success code")]
    SuccessCodeRequired,
    /// An error constructor received a success code.
    #[error("error response requires a failure code")]
    FailureCodeRequired,
    /// A deserialized success response included error metadata.
    #[error("success response cannot contain error metadata")]
    SuccessHasError,
    /// A deserialized failure included success data, including explicit null.
    #[error("error response cannot contain success data")]
    FailureHasData,
}

/// Empty JSON object used for `data: {}`, distinct from omitted data or JSON null.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ApiEmptyData {}

/// Product-neutral error metadata with a typed product diagnostic extension.
/// Products decide retryability, diagnostic visibility, classification and redaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(deserialize = "D: Deserialize<'de>"))]
pub struct ApiErrorInfo<D = ()> {
    /// A product decision, not an instruction to automatically retry.
    pub retryable: bool,
    /// Omitted when absent. Use a product struct instead of unvalidated JSON for extensions.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub diagnostic: Option<D>,
}

/// REST wire contract: `{code,msg,data?,error?}`.
///
/// Fields are private; constructors and deserialization enforce code/state consistency.
/// This type has no web-framework dependency and never chooses a status, header or error message.
///
/// ```
/// use aster_forge_api::response::{ApiResponse, ApiResponseCode};
/// #[derive(serde::Serialize)]
/// enum ProductCode { Success }
/// impl ApiResponseCode for ProductCode {
///     fn is_success(&self) -> bool { matches!(self, Self::Success) }
/// }
/// let response = ApiResponse::<_, ProductCode>::ok(ProductCode::Success, vec![1, 2])?;
/// assert_eq!(response.data(), Some(&vec![1, 2]));
/// # Ok::<(), aster_forge_api::response::ResponseEnvelopeError>(())
/// ```
///
/// Callers cannot bypass state validation with a struct literal:
/// ```compile_fail
/// use aster_forge_api::response::ApiResponse;
/// let response = ApiResponse::<(), u8> {
///     code: 0, msg: String::new(), data: Some(()), error: None,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiResponse<T, C, D = ()> {
    code: C,
    msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ApiErrorInfo<D>>,
}

impl<T, C: ApiResponseCode, D> ApiResponse<T, C, D> {
    /// Builds success with data and an empty message, preserving a null-valued `T` as data.
    ///
    /// # Errors
    /// Returns [`ResponseEnvelopeError::SuccessCodeRequired`] for a failure code.
    pub fn ok(code: C, data: T) -> Result<Self, ResponseEnvelopeError> {
        Self::ok_with_message(code, "", data)
    }

    /// Builds success with a product-provided message and data.
    ///
    /// # Errors
    /// Returns [`ResponseEnvelopeError::SuccessCodeRequired`] for a failure code.
    pub fn ok_with_message(
        code: C,
        message: impl Into<String>,
        data: T,
    ) -> Result<Self, ResponseEnvelopeError> {
        if !code.is_success() {
            return Err(ResponseEnvelopeError::SuccessCodeRequired);
        }
        Ok(Self {
            code,
            msg: message.into(),
            data: Some(data),
            error: None,
        })
    }
}

impl<C: ApiResponseCode, D> ApiResponse<(), C, D> {
    /// Builds success with an empty message and omitted data.
    ///
    /// # Errors
    /// Returns [`ResponseEnvelopeError::SuccessCodeRequired`] for a failure code.
    pub fn ok_empty(code: C) -> Result<Self, ResponseEnvelopeError> {
        if !code.is_success() {
            return Err(ResponseEnvelopeError::SuccessCodeRequired);
        }
        Ok(Self {
            code,
            msg: String::new(),
            data: None,
            error: None,
        })
    }

    /// Builds a failure with `retryable: false` and no diagnostic, matching AD's ordinary error.
    ///
    /// # Errors
    /// Returns [`ResponseEnvelopeError::FailureCodeRequired`] for a success code.
    pub fn error(code: C, message: impl Into<String>) -> Result<Self, ResponseEnvelopeError> {
        Self::error_with_details(
            code,
            message,
            Some(ApiErrorInfo {
                retryable: false,
                diagnostic: None,
            }),
        )
    }

    /// Builds a failure with optional product error metadata. `None` omits the error field.
    ///
    /// # Errors
    /// Returns [`ResponseEnvelopeError::FailureCodeRequired`] for a success code.
    pub fn error_with_details(
        code: C,
        message: impl Into<String>,
        error: Option<ApiErrorInfo<D>>,
    ) -> Result<Self, ResponseEnvelopeError> {
        if code.is_success() {
            return Err(ResponseEnvelopeError::FailureCodeRequired);
        }
        Ok(Self {
            code,
            msg: message.into(),
            data: None,
            error,
        })
    }
}

impl<C: ApiResponseCode, D> ApiResponse<ApiEmptyData, C, D> {
    /// Builds success with `data: {}` and an empty message.
    ///
    /// # Errors
    /// Returns [`ResponseEnvelopeError::SuccessCodeRequired`] for a failure code.
    pub fn ok_empty_data(code: C) -> Result<Self, ResponseEnvelopeError> {
        Self::ok(code, ApiEmptyData::default())
    }
}

impl<T, C, D> ApiResponse<T, C, D> {
    /// The product's typed code.
    #[must_use]
    pub const fn code(&self) -> &C {
        &self.code
    }
    /// Product-provided message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.msg
    }
    /// Data if the success includes a data field.
    #[must_use]
    pub const fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }
    /// Optional product error metadata.
    #[must_use]
    pub const fn error_info(&self) -> Option<&ApiErrorInfo<D>> {
        self.error.as_ref()
    }
}

// Missing fields are None, while explicit null is deserialized as T (e.g. (), Option<DTO>).
// This preserves AD's distinction between omitted data, null data and empty-object data.
fn present<'de, T: Deserialize<'de>, E: Deserializer<'de>>(
    deserializer: E,
) -> Result<Option<T>, E::Error> {
    T::deserialize(deserializer).map(Some)
}

impl<'de, T, C, D> Deserialize<'de> for ApiResponse<T, C, D>
where
    T: Deserialize<'de>,
    C: Deserialize<'de> + ApiResponseCode,
    D: Deserialize<'de>,
{
    fn deserialize<E: Deserializer<'de>>(deserializer: E) -> Result<Self, E::Error> {
        #[derive(Deserialize)]
        #[serde(bound(
            deserialize = "T: Deserialize<'de>, C: Deserialize<'de>, D: Deserialize<'de>"
        ))]
        struct Wire<T, C, D> {
            code: C,
            msg: String,
            #[serde(default, deserialize_with = "present")]
            data: Option<T>,
            #[serde(default, deserialize_with = "present")]
            error: Option<ApiErrorInfo<D>>,
        }
        let wire: Wire<T, C, D> = Wire::deserialize(deserializer)?;
        if wire.code.is_success() && wire.error.is_some() {
            return Err(E::Error::custom(ResponseEnvelopeError::SuccessHasError));
        }
        if !wire.code.is_success() && wire.data.is_some() {
            return Err(E::Error::custom(ResponseEnvelopeError::FailureHasData));
        }
        Ok(Self {
            code: wire.code,
            msg: wire.msg,
            data: wire.data,
            error: wire.error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
    enum DriveCode {
        #[serde(rename = "success")]
        Success,
        #[serde(rename = "auth.credentials_failed")]
        CredentialsFailed,
        #[serde(rename = "rate_limited")]
        RateLimited,
    }
    impl ApiResponseCode for DriveCode {
        fn is_success(&self) -> bool {
            *self == Self::Success
        }
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
    enum GateCode {
        #[serde(rename = "success")]
        Success,
        #[serde(rename = "account.identifier_conflict")]
        IdentifierConflict,
    }
    impl ApiResponseCode for GateCode {
        fn is_success(&self) -> bool {
            *self == Self::Success
        }
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
    struct Diagnostic {
        kind: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
    struct Profile {
        id: u64,
        name: String,
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
    struct Task {
        id: u64,
        completed: bool,
    }

    #[cfg(all(debug_assertions, feature = "openapi"))]
    #[derive(Serialize, Deserialize, utoipa::ToSchema)]
    struct Group {
        owner: Profile,
    }

    fn round_trip<T, C, D>(response: &ApiResponse<T, C, D>, expected: &Value)
    where
        T: Serialize + for<'a> Deserialize<'a>,
        C: ApiResponseCode + Serialize + for<'a> Deserialize<'a>,
        D: Serialize + for<'a> Deserialize<'a>,
    {
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(&value, expected);
        let decoded: ApiResponse<T, C, D> = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }

    #[test]
    fn drive_wire_contract_distinguishes_data_absent_null_and_empty_object() {
        round_trip(
            &ApiResponse::<_, DriveCode>::ok(
                DriveCode::Success,
                Profile {
                    id: 7,
                    name: "cat".into(),
                },
            )
            .unwrap(),
            &json!({"code":"success","msg":"","data":{"id":7,"name":"cat"}}),
        );
        round_trip(
            &ApiResponse::<(), DriveCode>::ok_empty(DriveCode::Success).unwrap(),
            &json!({"code":"success","msg":""}),
        );
        round_trip(
            &ApiResponse::<(), DriveCode>::ok(DriveCode::Success, ()).unwrap(),
            &json!({"code":"success","msg":"","data":null}),
        );
        round_trip(
            &ApiResponse::<Option<Profile>, DriveCode>::ok(DriveCode::Success, None).unwrap(),
            &json!({"code":"success","msg":"","data":null}),
        );
        round_trip(
            &ApiResponse::<ApiEmptyData, DriveCode>::ok_empty_data(DriveCode::Success).unwrap(),
            &json!({"code":"success","msg":"","data":{}}),
        );
    }

    #[test]
    fn drive_failures_preserve_optional_metadata_and_typed_diagnostic() {
        round_trip(
            &ApiResponse::<(), DriveCode>::error(
                DriveCode::CredentialsFailed,
                "Invalid Credentials",
            )
            .unwrap(),
            &json!({"code":"auth.credentials_failed","msg":"Invalid Credentials","error":{"retryable":false}}),
        );
        round_trip(
            &ApiResponse::<(), DriveCode>::error_with_details(
                DriveCode::CredentialsFailed,
                "Invalid Credentials",
                None,
            )
            .unwrap(),
            &json!({"code":"auth.credentials_failed","msg":"Invalid Credentials"}),
        );
        let response = ApiResponse::<(), DriveCode, Diagnostic>::error_with_details(
            DriveCode::RateLimited,
            "Too many requests",
            Some(ApiErrorInfo {
                retryable: true,
                diagnostic: Some(Diagnostic {
                    kind: "transient".into(),
                    message: "retry later".into(),
                    field: None,
                    scope: None,
                }),
            }),
        )
        .unwrap();
        round_trip(
            &response,
            &json!({"code":"rate_limited","msg":"Too many requests","error":{"retryable":true,
            "diagnostic":{"kind":"transient","message":"retry later"}}}),
        );
        assert!(response.data().is_none());
        assert_eq!(response.code(), &DriveCode::RateLimited);
        assert_eq!(response.message(), "Too many requests");
        assert!(response.error_info().unwrap().retryable);
    }

    #[test]
    fn independent_product_code_and_pagination_compose_without_product_semantics() {
        let response = ApiResponse::<_, GateCode>::ok_with_message(
            GateCode::Success,
            "product message",
            crate::OffsetPage::new(
                vec![Profile {
                    id: 1,
                    name: "one".into(),
                }],
                1,
                20,
                0,
            ),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({"code":"success","msg":"product message",
            "data":{"items":[{"id":1,"name":"one"}],"total":1,"limit":20,"offset":0}})
        );
        round_trip(
            &ApiResponse::<(), GateCode>::error(GateCode::IdentifierConflict, "Already used")
                .unwrap(),
            &json!({"code":"account.identifier_conflict","msg":"Already used","error":{"retryable":false}}),
        );
    }

    #[test]
    fn constructors_reject_codes_for_the_wrong_state() {
        assert_eq!(
            ApiResponse::<(), DriveCode>::ok(DriveCode::CredentialsFailed, ()).unwrap_err(),
            ResponseEnvelopeError::SuccessCodeRequired
        );
        assert_eq!(
            ApiResponse::<(), DriveCode>::ok_empty(DriveCode::CredentialsFailed).unwrap_err(),
            ResponseEnvelopeError::SuccessCodeRequired
        );
        assert_eq!(
            ApiResponse::<ApiEmptyData, GateCode>::ok_empty_data(GateCode::IdentifierConflict)
                .unwrap_err(),
            ResponseEnvelopeError::SuccessCodeRequired
        );
        assert_eq!(
            ApiResponse::<(), DriveCode>::error(DriveCode::Success, "bad").unwrap_err(),
            ResponseEnvelopeError::FailureCodeRequired
        );
        assert_eq!(
            ApiResponse::<Task, DriveCode>::ok_with_message(
                DriveCode::RateLimited,
                "bad",
                Task {
                    id: 0,
                    completed: false
                }
            )
            .unwrap_err(),
            ResponseEnvelopeError::SuccessCodeRequired
        );
        assert_eq!(
            ApiResponse::<(), GateCode>::error_with_details(GateCode::Success, "bad", None)
                .unwrap_err(),
            ResponseEnvelopeError::FailureCodeRequired
        );
    }

    #[test]
    fn absence_and_null_are_preserved_for_nested_data_and_diagnostic_types() {
        for value in [
            json!({"code":"success","msg":""}),
            json!({"code":"success","msg":"","data":null}),
            json!({"code":"success","msg":"","data":[]}),
        ] {
            let response: ApiResponse<Option<Vec<Profile>>, DriveCode> =
                serde_json::from_value(value.clone()).unwrap();
            assert_eq!(response.data().is_some(), value.get("data").is_some());
            assert_eq!(serde_json::to_value(response).unwrap(), value);
        }
        for value in [
            json!({"code":"rate_limited","msg":"wait","error":{"retryable":true}}),
            json!({"code":"rate_limited","msg":"wait","error":{"retryable":true,"diagnostic":null}}),
        ] {
            let response: ApiResponse<(), DriveCode> =
                serde_json::from_value(value.clone()).unwrap();
            assert_eq!(
                response.error_info().unwrap().diagnostic.is_some(),
                value["error"].get("diagnostic").is_some()
            );
            assert_eq!(serde_json::to_value(response).unwrap(), value);
        }
    }

    #[test]
    fn empty_collections_unicode_and_numeric_bounds_are_not_normalized_or_truncated() {
        round_trip(
            &ApiResponse::<_, GateCode>::ok(GateCode::Success, Vec::<Profile>::new()).unwrap(),
            &json!({"code":"success","msg":"","data":[]}),
        );
        let response = ApiResponse::<_, DriveCode>::ok_with_message(
            DriveCode::Success,
            "猫猫 🐈\n\t",
            u64::MAX,
        )
        .unwrap();
        round_trip(
            &response,
            &json!({"code":"success","msg":"猫猫 🐈\n\t","data":u64::MAX}),
        );
        let response: ApiResponse<&str, GateCode> =
            serde_json::from_str(r#"{"code":"success","msg":"","data":"borrowed"}"#).unwrap();
        assert_eq!(response.data(), Some(&"borrowed"));
    }

    #[test]
    fn product_diagnostic_field_and_scope_are_typed_and_omitted_independently() {
        let diagnostic = Diagnostic {
            kind: "connector_validation".into(),
            message: "invalid setting".into(),
            field: Some("endpoint".into()),
            scope: Some("connector_config".into()),
        };
        let response = ApiResponse::<(), DriveCode, Diagnostic>::error_with_details(
            DriveCode::CredentialsFailed,
            "product policy",
            Some(ApiErrorInfo {
                retryable: false,
                diagnostic: Some(diagnostic),
            }),
        )
        .unwrap();
        round_trip(
            &response,
            &json!({"code":"auth.credentials_failed","msg":"product policy","error":{"retryable":false,
            "diagnostic":{"kind":"connector_validation","message":"invalid setting","field":"endpoint","scope":"connector_config"}}}),
        );
    }

    #[test]
    fn state_errors_are_reported_after_valid_dto_deserialization() {
        for value in [
            json!({"code":"auth.credentials_failed","msg":"bad","data":null}),
            json!({"code":"auth.credentials_failed","msg":"bad","data":{"id":7,"name":"cat"}}),
        ] {
            let error = serde_json::from_value::<ApiResponse<Option<Profile>, DriveCode>>(value)
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&ResponseEnvelopeError::FailureHasData.to_string())
            );
        }
        let error = serde_json::from_value::<ApiResponse<Profile, GateCode>>(
            json!({"code":"success","msg":"","error":{"retryable":false}}),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&ResponseEnvelopeError::SuccessHasError.to_string())
        );
    }

    #[test]
    fn malformed_fields_and_product_diagnostics_fail_instead_of_becoming_unknown_json() {
        for value in [
            json!({"code":"success","msg":null}),
            json!({"code":"success","msg":"","data":null}),
            json!({"code":"auth.credentials_failed","msg":"bad","error":null}),
            json!({"code":"auth.credentials_failed","msg":"bad","error":{}}),
            json!({"code":"auth.credentials_failed","msg":"bad","error":{"retryable":"false"}}),
            json!({"code":"auth.credentials_failed","msg":"bad","error":{"retryable":false,"diagnostic":{"kind":7,"message":"bad"}}}),
        ] {
            assert!(
                serde_json::from_value::<ApiResponse<Profile, DriveCode, Diagnostic>>(value)
                    .is_err()
            );
        }
        assert!(
            serde_json::from_str::<ApiResponse<(), DriveCode>>(
                r#"{"code":"success","code":"rate_limited","msg":""}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<ApiResponse<(), GateCode>>(
                r#"{"code":"account.identifier_conflict","msg":"","data":null,"data":null}"#
            )
            .is_err()
        );
    }

    #[test]
    fn deserialization_rejects_contradictions_and_unknown_product_codes() {
        for value in [
            json!({"code":"success","msg":"","error":{"retryable":false}}),
            json!({"code":"auth.credentials_failed","msg":"bad","data":null}),
            json!({"code":"auth.credentials_failed","msg":"bad","data":{}}),
            json!({"code":"unknown.code","msg":"bad"}),
            json!({"code":"success"}),
        ] {
            assert!(
                serde_json::from_value::<ApiResponse<Option<Profile>, DriveCode>>(value).is_err()
            );
        }
    }

    #[cfg(all(debug_assertions, feature = "openapi"))]
    fn verify_refs(value: &Value, schemas: &serde_json::Map<String, Value>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref") {
                    let name = reference.strip_prefix("#/components/schemas/").unwrap();
                    assert!(schemas.contains_key(name), "unresolved schema {name}");
                }
                for child in map.values() {
                    verify_refs(child, schemas);
                }
            }
            Value::Array(values) => {
                for child in values {
                    verify_refs(child, schemas);
                }
            }
            _ => (),
        }
    }

    #[cfg(all(debug_assertions, feature = "openapi"))]
    #[test]
    fn generic_openapi_names_references_and_typed_codes_are_distinct() {
        use utoipa::{OpenApi, ToSchema};
        type NoData = ();
        #[derive(OpenApi)]
        #[openapi(components(schemas(
            ApiResponse<Profile, DriveCode, Diagnostic>,
            ApiResponse<Task, DriveCode, Diagnostic>,
            ApiResponse<Profile, GateCode, NoData>,
            ApiResponse<Task, GateCode, NoData>,
            ApiResponse<Group, GateCode, NoData>,
            ApiResponse<NoData, DriveCode, Diagnostic>,
            ApiResponse<NoData, GateCode, NoData>,
            ApiErrorInfo<NoData>
        )))]
        struct ApiDoc;

        let doc = serde_json::to_value(ApiDoc::openapi()).unwrap();
        if let Ok(path) = std::env::var("ASTER_FORGE_RESPONSE_OPENAPI_OUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
        }
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        assert_eq!(
            schemas["ApiErrorInfo_TupleUnit"]["properties"]["diagnostic"]["type"],
            "null"
        );
        assert!(
            schemas["ApiErrorInfo_TupleUnit"]["properties"]["diagnostic"]
                .get("default")
                .is_none()
        );
        assert_eq!(
            schemas["ApiResponse_TupleUnit_GateCode_TupleUnit"]["properties"]["data"]["type"],
            "null"
        );
        assert!(
            schemas["ApiResponse_TupleUnit_GateCode_TupleUnit"]["properties"]["data"]
                .get("default")
                .is_none()
        );
        let names = [
            (Profile::name(), DriveCode::name(), Diagnostic::name()),
            (Task::name(), DriveCode::name(), Diagnostic::name()),
            (Profile::name(), GateCode::name(), NoData::name()),
            (Task::name(), GateCode::name(), NoData::name()),
            (Group::name(), GateCode::name(), NoData::name()),
            (NoData::name(), DriveCode::name(), Diagnostic::name()),
            (NoData::name(), GateCode::name(), NoData::name()),
        ]
        .map(|(data, code, diagnostic)| format!("ApiResponse_{data}_{code}_{diagnostic}"));
        assert_eq!(
            names
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            names.len()
        );
        for name in names {
            assert!(schemas.contains_key(&name), "missing {name}");
            let expected = if name.contains("DriveCode") {
                json!(["success", "auth.credentials_failed", "rate_limited"])
            } else {
                json!(["success", "account.identifier_conflict"])
            };
            let code_name = if name.contains("DriveCode") {
                "DriveCode"
            } else {
                "GateCode"
            };
            assert_eq!(
                schemas[&name]["properties"]["code"]["$ref"],
                format!("#/components/schemas/{code_name}")
            );
            assert_eq!(schemas[code_name]["enum"], expected);
            if name.contains("Profile") {
                assert!(
                    schemas[&name]["properties"]["data"]["properties"]
                        .get("name")
                        .is_some()
                );
                assert!(
                    schemas[&name]["properties"]["data"]["properties"]
                        .get("completed")
                        .is_none()
                );
            }
            if name.contains("Task") {
                assert!(
                    schemas[&name]["properties"]["data"]["properties"]
                        .get("completed")
                        .is_some()
                );
                assert!(
                    schemas[&name]["properties"]["data"]["properties"]
                        .get("name")
                        .is_none()
                );
            }
        }

        verify_refs(&doc, schemas);
    }
}
