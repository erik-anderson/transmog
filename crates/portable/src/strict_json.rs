use serde::{
    Deserializer,
    de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};
use std::fmt;

pub(crate) fn read(bytes: &[u8]) -> Result<Value, crate::Error> {
    if bytes.len() > 8 * 1024 * 1024 || bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(crate::invalid("Manifest exceeds eight MiB or has a BOM"));
    }
    let mut input = serde_json::Deserializer::from_slice(bytes);
    let result = Depth(0).deserialize(&mut input)?;
    input.end()?;
    Ok(result)
}

struct Depth(usize);
impl<'de> DeserializeSeed<'de> for Depth {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, input: D) -> Result<Value, D::Error> {
        if self.0 > 32 {
            return Err(D::Error::custom("JSON nesting exceeds 32"));
        }
        input.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Depth {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded strict JSON")
    }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        // serde_json falls back to f64 for overflowing integer tokens. Do not
        // silently preserve rounded descriptive integers as if they were exact.
        if value >= 18_446_744_073_709_551_616.0 || value <= -9_223_372_036_854_775_808.0 {
            return Err(E::custom("Large JSON numbers must use decimal strings"));
        }
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("Non-finite number"))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = input.next_element_seed(Depth(self.0 + 1))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = input.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("Duplicate JSON key"));
            }
            values.insert(key, input.next_value_seed(Depth(self.0 + 1))?);
        }
        Ok(Value::Object(values))
    }
}
