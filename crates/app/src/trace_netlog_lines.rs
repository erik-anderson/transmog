//! Chromium's typed newline-delimited `NetLog` records share the ordinary importer.
use super::{AppError, AtomicBool, Ordering, Value, invalid, unavailable};
use std::io::Read;

fn parse_error(canceled: &AtomicBool) -> AppError {
    if canceled.load(Ordering::Acquire) {
        unavailable("Trace import canceled")
    } else {
        invalid("The trace is not valid JSON or Chromium NetLog JSONL")
    }
}

pub(super) fn read(
    reader: impl Read,
    canceled: &AtomicBool,
) -> Result<(Value, Vec<String>), AppError> {
    if canceled.load(Ordering::Acquire) {
        return Err(unavailable("Trace import canceled"));
    }
    let mut records = serde_json::Deserializer::from_reader(reader).into_iter::<Value>();
    let mut first = records
        .next()
        .ok_or_else(|| invalid("The JSON trace is empty"))?
        .map_err(|_| parse_error(canceled))?;
    if first["type"] != "constants" {
        if records.next().is_some() {
            return Err(parse_error(canceled));
        }
        return Ok((first, Vec::new()));
    }
    if !first["constants"].is_object() {
        return Err(invalid("NetLog JSONL constants are missing"));
    }
    let mut document = serde_json::json!({"constants":first["constants"].take(),"events":[]});
    let mut events = Vec::new();
    let mut ended = false;
    let mut polled = false;
    let mut notes = Vec::new();
    for record in records {
        if canceled.load(Ordering::Acquire) {
            return Err(unavailable("Trace import canceled"));
        }
        let mut record = match record {
            Ok(record) => record,
            Err(error) if error.is_eof() && !ended => {
                notes.push("NetLog JSONL ended inside its final record. Complete preceding records were recovered; the incomplete tail was ignored.".into());
                break;
            }
            Err(_) => return Err(parse_error(canceled)),
        };
        if ended {
            return Err(invalid(
                "NetLog JSONL contains records after its end marker",
            ));
        }
        match record["type"].as_str() {
            Some("event") if !polled && record["event"].is_object() => {
                events.push(record["event"].take());
            }
            Some("polledData") if !polled && record.get("polledData").is_some() => {
                document["polledData"] = record["polledData"].take();
                polled = true;
            }
            Some("end") => ended = true,
            _ => {
                return Err(invalid(
                    "NetLog JSONL contains an invalid, duplicated or out-of-order record",
                ));
            }
        }
    }
    if !ended {
        notes.push("NetLog JSONL has no end marker. The available event prefix was imported; request completion still depends on each recorded request end.".into());
    }
    document["events"] = Value::Array(events);
    document["fileFormat"] = Value::String("jsonl".into());
    Ok((document, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_records_preserve_constants_events_polled_data_and_end() {
        let input=b"{\"type\":\"constants\",\"constants\":{\"logFormatVersion\":1}}\r\n\n{\"type\":\"event\",\"event\":{\"source\":{\"id\":12},\"type\":4}}\n{\"type\":\"polledData\",\"polledData\":{\"network\":\"placeholder\"}}\n{\"type\":\"end\"}\n";
        let (document, notes) = read(input.as_slice(), &AtomicBool::new(false)).unwrap();
        assert_eq!(notes, [] as [std::string::String; 0]);
        assert_eq!(document["constants"]["logFormatVersion"], 1);
        assert_eq!(document["events"][0]["source"]["id"], 12);
        assert_eq!(document["polledData"]["network"], "placeholder");
    }
    #[test]
    fn interrupted_tail_recovers_complete_events_but_malformed_records_are_atomic_failures() {
        let prefix = "{\"type\":\"constants\",\"constants\":{}}\n{\"type\":\"event\",\"event\":{\"type\":4}}\n";
        let (document, notes) = read(
            format!("{prefix}{{\"type\":\"event\",\"event\":").as_bytes(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(document["events"].as_array().unwrap().len(), 1);
        assert_eq!(notes.len(), 2);
        for tail in [
            "{broken}\n{\"type\":\"end\"}",
            "{\"type\":\"constants\",\"constants\":{}}",
            "{\"type\":\"end\"}\n{\"type\":\"event\",\"event\":{}}",
        ] {
            assert!(
                read(
                    format!("{prefix}{tail}").as_bytes(),
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
    }
}
