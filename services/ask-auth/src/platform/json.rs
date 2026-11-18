use crate::platform::AppError;
use serde::{
    de::{value::MapAccessDeserializer, DeserializeOwned, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{fmt, marker::PhantomData};

pub(crate) struct ObjectOnly<T>(pub(crate) T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for ObjectOnly<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}
struct ObjectVisitor<T>(PhantomData<T>);
impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
    type Value = ObjectOnly<T>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a configuration object")
    }
    fn visit_map<A: MapAccess<'de>>(self, entries: A) -> Result<Self::Value, A::Error> {
        T::deserialize(MapAccessDeserializer::new(entries)).map(ObjectOnly)
    }
}
pub(crate) fn parse<T: DeserializeOwned>(text: &str) -> Result<T, AppError> {
    if text.len() > 256 * 1024 {
        return Err(AppError::Configuration);
    }
    crate::json::check_json_depth(text.as_bytes()).map_err(|_| AppError::Configuration)?;
    serde_json::from_str::<ObjectOnly<T>>(text)
        .map(|value| value.0)
        .map_err(|_| AppError::Configuration)
}
