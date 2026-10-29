//! Validation uses the same bounded contracts advertised during discovery.
use crate::Error;
use serde_json::Value;

pub(crate) fn validate(schema: &Value, value: &Value) -> Result<(), Error> {
    check(schema, value, "arguments", 0)
}

fn fail(path: &str, message: &str) -> Error {
    Error::new("invalid_input", format!("{path}: {message}"))
}

fn check(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), Error> {
    if depth > 40 {
        return Err(fail(path, "nesting exceeds 40 levels."));
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        if let Some(option) = options.iter().find(|option| {
            option["properties"].as_object().is_some_and(|fields| {
                fields.iter().any(|(name, field)| {
                    field
                        .get("const")
                        .is_some_and(|expected| value.get(name) == Some(expected))
                })
            })
        }) {
            return check(option, value, path, depth + 1);
        }
        if options
            .iter()
            .any(|option| check(option, value, path, depth + 1).is_ok())
        {
            return Ok(());
        }
        return Err(fail(
            path,
            "does not match a supported shape; inspect the tool schema.",
        ));
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        return Err(fail(path, "has an unsupported value."));
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array)
        && !choices.contains(value)
    {
        return Err(fail(path, "choose one of the advertised values."));
    }
    match schema["type"].as_str() {
        Some("object") => check_object(schema, value, path, depth)?,
        Some("array") => {
            let items = value
                .as_array()
                .ok_or_else(|| fail(path, "must be an array."))?;
            bounds(schema, items.len() as u64, "minItems", "maxItems", path)?;
            for (index, item) in items.iter().enumerate() {
                check(
                    &schema["items"],
                    item,
                    &format!("{path}[{index}]"),
                    depth + 1,
                )?;
            }
            if schema["uniqueItems"] == true {
                for (index, item) in items.iter().enumerate() {
                    if items[..index].contains(item) {
                        return Err(fail(path, "contains duplicates."));
                    }
                }
            }
        }
        Some("string") => {
            let text = value.as_str().ok_or_else(|| fail(path, "must be text."))?;
            bounds(
                schema,
                text.chars().count() as u64,
                "minLength",
                "maxLength",
                path,
            )?;
            if let Some(pattern) = schema["pattern"].as_str() {
                let regex = regex::Regex::new(pattern)
                    .map_err(|_| fail(path, "has an invalid contract."))?;
                if !regex.is_match(text) {
                    return Err(fail(path, "does not match the advertised format."));
                }
            }
        }
        Some("integer")
            if schema.get("minimum").is_none()
                && schema.get("maximum").is_none()
                && (value.is_i64() || value.is_u64()) => {}
        Some("integer") => bounds(
            schema,
            value
                .as_u64()
                .ok_or_else(|| fail(path, "must be a nonnegative integer."))?,
            "minimum",
            "maximum",
            path,
        )?,
        Some("boolean") if !value.is_boolean() => return Err(fail(path, "must be true or false.")),
        Some("null") if !value.is_null() => return Err(fail(path, "must be null.")),
        _ => {}
    }
    Ok(())
}

fn bounds(schema: &Value, value: u64, min: &str, max: &str, path: &str) -> Result<(), Error> {
    if schema[min].as_u64().is_some_and(|bound| value < bound)
        || schema[max].as_u64().is_some_and(|bound| value > bound)
    {
        return Err(fail(path, "is outside the advertised size or range."));
    }
    Ok(())
}

fn check_object(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), Error> {
    let fields = value
        .as_object()
        .ok_or_else(|| fail(path, "must be an object."))?;
    if let Some(required) = schema["required"].as_array() {
        for name in required.iter().filter_map(Value::as_str) {
            if !fields.contains_key(name) {
                return Err(fail(&format!("{path}.{name}"), "is required."));
            }
        }
    }
    for (name, field) in fields {
        let child = format!("{path}.{name}");
        if let Some(property) = schema
            .get("properties")
            .and_then(|properties| properties.get(name))
        {
            check(property, field, &child, depth + 1)?;
        } else if schema["additionalProperties"] == false {
            return Err(fail(&child, "is unsupported."));
        }
    }

    Ok(())
}
