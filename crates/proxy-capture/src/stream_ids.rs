//! Persistent SSE IDs are defined once and referenced by their defining record index.
use crate::{CaptureError, FrameCodec, StoredRecord};
use std::{collections::HashMap, sync::Arc};

#[derive(Default)]
pub(crate) struct WriterIds {
    values: HashMap<Arc<str>, u64>,
    last: Option<(Arc<str>, u64)>,
}
impl WriterIds {
    pub(crate) fn store(&mut self, value: &Arc<str>, index: u64) -> (u64, bool) {
        // The SSE parser shares its persistent ID. Avoid hashing a potentially
        // large unchanged value on every heartbeat.
        if let Some((last, reference)) = &self.last
            && Arc::ptr_eq(last, value)
        {
            return (*reference, false);
        }
        let reference = *self.values.entry(value.clone()).or_insert(index);
        self.last = Some((value.clone(), reference));
        (reference, reference == index)
    }
}

#[derive(Default)]
pub(crate) struct ReaderIds(HashMap<u64, Arc<str>>);
impl ReaderIds {
    pub(crate) fn prepare(
        &mut self,
        stored: &StoredRecord,
        index: u64,
        codec: &mut FrameCodec,
    ) -> Result<(), CaptureError> {
        codec.stream_id = None;
        let Some(reference) = stored.stream_id else {
            return Ok(());
        };
        if stored.kind != "stream-message" || !stored.payload.is_object() || reference > index {
            return Err(CaptureError::MalformedRecord("invalid stream ID reference"));
        }
        let value = if reference == index {
            let value: Arc<str> = stored.payload["eventId"]
                .as_str()
                .ok_or(CaptureError::MalformedRecord(
                    "missing stream ID definition",
                ))?
                .into();
            self.0.insert(index, value.clone());
            value
        } else {
            if !stored.payload["eventId"].is_null() {
                return Err(CaptureError::MalformedRecord(
                    "conflicting stream ID definition",
                ));
            }
            self.0
                .get(&reference)
                .cloned()
                .ok_or(CaptureError::MalformedRecord("missing stream ID reference"))?
        };
        // Pin only this value for random payload reads, never the whole dictionary.
        codec.stream_id = Some((reference, value));
        Ok(())
    }
}

pub(crate) fn resolve(
    stored: &mut StoredRecord,
    codec: &FrameCodec,
) -> Result<Option<Arc<str>>, CaptureError> {
    let Some(reference) = stored.stream_id.take() else {
        return Ok(None);
    };
    let Some((pinned, value)) = &codec.stream_id else {
        return Err(CaptureError::MalformedRecord("missing pinned stream ID"));
    };
    if stored.kind != "stream-message" || !stored.payload.is_object() || *pinned != reference {
        return Err(CaptureError::MalformedRecord(
            "mismatched stream ID reference",
        ));
    }
    if let Some(inline) = stored
        .payload
        .get("eventId")
        .filter(|value| !value.is_null())
        && inline.as_str() != Some(value.as_ref())
    {
        return Err(CaptureError::MalformedRecord(
            "mismatched stream ID definition",
        ));
    }
    // Do not deserialize another allocation of the defining message's ID.
    stored.payload["eventId"] = serde_json::Value::Null;
    Ok(Some(value.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CaptureEncoding, CaptureRecord, CaptureRecordKind, store};
    use transmog_core::stream::StreamMessage;

    #[test]
    fn definitions_references_resets_and_invalid_links() {
        let (mut codec, _) =
            FrameCodec::create(&mut Vec::new(), &CaptureEncoding::default()).unwrap();
        let mut ids = ReaderIds::default();
        let mut message = StreamMessage::new("server", "event", Vec::new().into());
        message.event_id = Some("persistent".into());
        let mut stored = store(&CaptureRecord {
            sequence: 1,
            exchange_id: 1,
            kind: CaptureRecordKind::StreamMessage(message),
        })
        .unwrap();
        stored.stream_id = Some(2);
        assert!(ids.prepare(&stored, 1, &mut codec).is_err());
        ids.prepare(&stored, 2, &mut codec).unwrap();
        let first = codec.stream_id.as_ref().unwrap().1.clone();
        assert!(ids.prepare(&stored, 3, &mut codec).is_err()); // References cannot redefine a value.
        stored.payload["eventId"] = serde_json::Value::Null;
        ids.prepare(&stored, 3, &mut codec).unwrap();
        let mut wrong = codec.clone();
        wrong.stream_id = Some((9, "other".into()));
        assert!(resolve(&mut stored.clone(), &wrong).is_err());
        let (record, _) = stored.clone().load_container_metadata(&codec).unwrap();
        let CaptureRecordKind::StreamMessage(message) = record.kind else {
            panic!("message")
        };
        assert!(Arc::ptr_eq(&first, message.event_id.as_ref().unwrap()));
        stored.stream_id = Some(1);
        assert!(ids.prepare(&stored, 4, &mut codec).is_err());
        stored.stream_id = Some(4);
        assert!(ids.prepare(&stored, 4, &mut codec).is_err()); // A definition must contain a string.
        stored.payload["eventId"] = "".into();
        ids.prepare(&stored, 4, &mut codec).unwrap(); // An empty ID is a real reset, not absence.
        assert_eq!(codec.stream_id.as_ref().unwrap().1.as_ref(), "");
        let mut scalar = stored.clone();
        scalar.payload = "invalid".into();
        assert!(ids.prepare(&scalar, 5, &mut codec).is_err());
        assert!(resolve(&mut scalar, &codec).is_err());
        stored.kind = "completed".into();
        assert!(ids.prepare(&stored, 5, &mut codec).is_err());
    }
}
