//! `OpenAPI` composition for envelopes, including typed unit values and nested schema collection.
use super::{ApiErrorInfo, ApiResponse};
use std::borrow::Cow;
use utoipa::{
    PartialSchema, ToSchema,
    openapi::{
        Ref, RefOr,
        schema::{ObjectBuilder, Schema, Type},
    },
};

// Utoipa's derived unit schema allows arbitrary values and adds a null default, which SDK
// generators turn into a required field. Serde's unit is exactly null and never supplies a default.
fn value_schema<T: ToSchema>() -> RefOr<Schema> {
    if std::any::type_name::<T>() == "()" {
        ObjectBuilder::new().schema_type(Type::Null).into()
    } else {
        T::schema()
    }
}

fn collect<T: ToSchema>(schemas: &mut Vec<(String, RefOr<Schema>)>) {
    schemas.push((T::name().into_owned(), value_schema::<T>()));
    T::schemas(schemas);
}

impl<D: ToSchema> ToSchema for ApiErrorInfo<D> {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("ApiErrorInfo")
    }
    fn schemas(schemas: &mut Vec<(String, RefOr<Schema>)>) {
        D::schemas(schemas);
    }
}

// ComposeSchema is the integration point used by Utoipa's generic derives. Keep this dependency
// detail private to this module; consumers use the usual ToSchema and full generic type syntax.
impl<D: ToSchema> utoipa::__dev::ComposeSchema for ApiErrorInfo<D> {
    fn compose(_generics: Vec<RefOr<Schema>>) -> RefOr<Schema> {
        ObjectBuilder::new()
            .property("retryable", bool::schema())
            .required("retryable")
            .property("diagnostic", value_schema::<D>())
            .into()
    }
}

impl<T: ToSchema, C: ToSchema, D: ToSchema> ToSchema for ApiResponse<T, C, D> {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("ApiResponse")
    }
    fn schemas(schemas: &mut Vec<(String, RefOr<Schema>)>) {
        T::schemas(schemas);
        collect::<C>(schemas);
        D::schemas(schemas);
    }
}

impl<T: ToSchema, C: ToSchema, D: ToSchema> utoipa::__dev::ComposeSchema for ApiResponse<T, C, D> {
    fn compose(_generics: Vec<RefOr<Schema>>) -> RefOr<Schema> {
        ObjectBuilder::new()
            .property("code", Ref::from_schema_name(C::name()))
            .required("code")
            .property("msg", String::schema())
            .required("msg")
            .property("data", value_schema::<T>())
            .property("error", ApiErrorInfo::<D>::schema())
            .into()
    }
}
