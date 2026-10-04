//! Omitted/null/value semantics for typed PATCH fields.
use serde::Deserialize;
/// Three-state nullable field used by PATCH-style request DTOs.
///
/// `Absent` means the request omitted the field and the existing value should be preserved.
/// `Null` means the request explicitly supplied `null` and the existing value should be cleared.
/// `Value` means the request supplied a concrete replacement value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NullablePatch<T> {
    /// Field was omitted from the request.
    #[default]
    Absent,
    /// Field was present with JSON `null`.
    Null,
    /// Field was present with a concrete value.
    Value(T),
}

impl<T> NullablePatch<T> {
    /// Returns whether the request supplied this field as either `null` or a concrete value.
    pub fn is_present(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// Deserializes an optional PATCH field while preserving explicit `null`.
///
/// Use this with `#[serde(default, deserialize_with = "...")]` on `Option<NullablePatch<T>>`
/// fields when the surrounding DTO needs to distinguish omitted fields from explicit nulls.
///
/// # Errors
///
/// Returns the deserializer's error when the present value cannot be deserialized as `T`.
pub fn deserialize_nullable_patch_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<NullablePatch<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(|value| Some(NullablePatch::from(value)))
}

impl<T> From<Option<T>> for NullablePatch<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        }
    }
}

impl<'de, T> Deserialize<'de> for NullablePatch<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patch_preserves_empty_values_and_rejects_wrong_types() {
        let dto: PatchDto = serde_json::from_value(json!({"title":"", "description":""})).unwrap();
        assert_eq!(dto.title, NullablePatch::Value(String::new()));
        assert_eq!(dto.description, Some(NullablePatch::Value(String::new())));
        for value in [
            json!({"title":42}),
            json!({"description":[]}),
            json!({"title":false}),
        ] {
            assert!(serde_json::from_value::<PatchDto>(value).is_err());
        }
    }
    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct PatchDto {
        #[serde(default)]
        title: NullablePatch<String>,
        #[serde(default, deserialize_with = "deserialize_nullable_patch_option")]
        description: Option<NullablePatch<String>>,
    }

    #[test]
    fn nullable_patch_deserializes_absent_null_and_value_fields() {
        let dto: PatchDto = serde_json::from_value(json!({})).unwrap();
        assert_eq!(dto.title, NullablePatch::Absent);
        assert_eq!(dto.description, None);
        assert!(!dto.title.is_present());

        let dto: PatchDto =
            serde_json::from_value(json!({ "title": null, "description": null })).unwrap();
        assert_eq!(dto.title, NullablePatch::Null);
        assert_eq!(dto.description, Some(NullablePatch::Null));
        assert!(dto.title.is_present());

        let dto: PatchDto = serde_json::from_value(json!({
            "title": "new title",
            "description": "new description"
        }))
        .unwrap();
        assert_eq!(dto.title, NullablePatch::Value("new title".to_string()));
        assert_eq!(
            dto.description,
            Some(NullablePatch::Value("new description".to_string()))
        );
    }
}
