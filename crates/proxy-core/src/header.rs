use http::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A validated header field that retains its original name bytes and position.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeaderField {
    name: Vec<u8>,
    value: Vec<u8>,
}

impl HeaderField {
    /// Constructs a field after validating HTTP header syntax.
    ///
    /// # Errors
    ///
    /// Returns [`HeaderError`] when the name or value is not legal HTTP header
    /// syntax.
    pub fn try_new(
        name: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<Self, HeaderError> {
        let name = name.into();
        let value = value.into();
        HeaderName::from_bytes(&name).map_err(|_| HeaderError::InvalidName)?;
        HeaderValue::from_bytes(&value).map_err(|_| HeaderError::InvalidValue)?;
        Ok(Self { name, value })
    }

    /// Returns the original field-name bytes.
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Returns the original field-value bytes.
    pub fn value(&self) -> &[u8] {
        &self.value
    }

    /// Tests a field name using ASCII case-insensitive comparison.
    pub fn name_eq(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name.as_bytes())
    }
}

/// An ordered header block that retains duplicates.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeaderBlock(Vec<HeaderField>);

impl HeaderBlock {
    /// Creates an empty block.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a block from already validated fields.
    pub fn from_fields(fields: Vec<HeaderField>) -> Self {
        Self(fields)
    }

    /// Appends a validated field.
    pub fn push(&mut self, field: HeaderField) {
        self.0.push(field);
    }

    /// Returns all fields in wire order.
    pub fn fields(&self) -> &[HeaderField] {
        &self.0
    }

    /// Iterates over fields in wire order.
    pub fn iter(&self) -> impl Iterator<Item = &HeaderField> {
        self.0.iter()
    }

    /// Returns every value for a name without combining duplicates.
    pub fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.0
            .iter()
            .filter(move |field| field.name_eq(name))
            .map(HeaderField::value)
    }

    /// Removes all fields with the supplied name and preserves other ordering.
    pub fn remove_all(&mut self, name: &str) {
        self.0.retain(|field| !field.name_eq(name));
    }

    /// Replaces all instances of a field at the position of its first occurrence.
    pub fn replace_all(&mut self, field: HeaderField) {
        let first = self
            .0
            .iter()
            .position(|item| item.name.eq_ignore_ascii_case(&field.name));
        self.0
            .retain(|item| !item.name.eq_ignore_ascii_case(&field.name));
        if let Some(index) = first {
            self.0.insert(index, field);
        } else {
            self.0.push(field);
        }
    }
}

/// Header validation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HeaderError {
    /// Field name is not an HTTP token.
    #[error("invalid HTTP header name")]
    InvalidName,
    /// Field value contains illegal bytes.
    #[error("invalid HTTP header value")]
    InvalidValue,
}

#[cfg(test)]
mod tests {
    use super::{HeaderBlock, HeaderField};

    #[test]
    fn duplicates_and_order_are_preserved() {
        let fields = vec![
            HeaderField::try_new("Set-Cookie", "a=1").unwrap(),
            HeaderField::try_new("X-Test", "middle").unwrap(),
            HeaderField::try_new("set-cookie", "b=2").unwrap(),
        ];
        let block = HeaderBlock::from_fields(fields);
        let values: Vec<_> = block.values("SET-cookie").collect();
        assert_eq!(values, vec![b"a=1".as_slice(), b"b=2".as_slice()]);
        assert_eq!(block.fields()[1].name(), b"X-Test");
    }
}
