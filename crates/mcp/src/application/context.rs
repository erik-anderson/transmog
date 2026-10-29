//! Key-owned complete context, paged independently of inline tool response budgets.
use super::*;
use std::time::Instant;

pub(super) struct Context {
    id: String,
    owner: String,
    pub(super) realm: String,
    revision: u64,
    expires: Instant,
    expires_at: u64,
    bytes: Arc<Vec<u8>>,
    required: Vec<Permission>,
    source: Value,
    metadata: Value,
    url: String,
    entry_id: Option<String>,
    representation: &'static str,
    text: bool,
}

pub(super) struct ContextInput {
    pub bytes: Vec<u8>,
    pub required: Vec<Permission>,
    pub source: Value,
    pub metadata: Value,
    pub url: String,
    pub entry_id: Option<String>,
    pub representation: &'static str,
    pub text: bool,
}

impl AgentApplication {
    pub(super) fn sweep_contexts(&self) {
        let access = self.access.status();
        self.context_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|context| {
                context.expires > Instant::now()
                    && access.enabled
                    && access.grants.iter().any(|grant| {
                        grant.id == context.owner
                            && grant.revision == context.revision
                            && !grant.revoked
                            && grant.expires_at.is_none_or(|expiry| expiry > now())
                    })
            });
    }
    pub(super) fn retain_context(
        &self,
        grant: &Grant,
        input: ContextInput,
    ) -> Result<Value, Error> {
        let ContextInput {
            bytes,
            required,
            source,
            metadata,
            url,
            entry_id,
            representation,
            text,
        } = input;
        if bytes.len() > transmog_app::MAX_CONTEXT_DECODE_BYTES {
            return Err(Error::new(
                "limit",
                "The context exceeds the 256 MiB whole-document budget. Retained byte-range reads remain available.",
            ));
        }
        let id = random(18)?;
        let expires_at = now() + 300;
        let mut contexts = self
            .context_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        contexts.retain(|context| context.expires > Instant::now());
        while contexts
            .iter()
            .filter(|context| context.owner == grant.id)
            .count()
            >= 64
            || contexts.len() >= 256
            || contexts
                .iter()
                .map(|context| context.bytes.len())
                .sum::<usize>()
                .saturating_add(bytes.len())
                > 512 * 1024 * 1024
        {
            let Some(index) = contexts
                .iter()
                .position(|context| context.owner == grant.id)
            else {
                return Err(Error::new(
                    "limit",
                    "The retained context budget is full. Retry after contexts expire.",
                ));
            };
            contexts.remove(index);
        }
        let length = bytes.len();
        contexts.push_back(Context {
            id: id.clone(),
            owner: grant.id.clone(),
            realm: self.session_id.clone(),
            revision: grant.revision,
            expires: Instant::now() + Duration::from_secs(300),
            expires_at,
            bytes: Arc::new(bytes),
            required,
            source,
            metadata,
            url,
            entry_id,
            representation,
            text,
        });
        Ok(
            json!({"contextId":id,"totalBytes":length.to_string(),"expiresAt":expires_at,"readWith":"transmog_context_read"}),
        )
    }

    pub(super) fn read_context(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        let contexts = self
            .context_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let context = contexts
            .iter()
            .find(|context| {
                context.realm == self.session_id
                    && context.id == input["contextId"]
                    && context.owner == grant.id
                    && context.revision == grant.revision
                    && context.expires > Instant::now()
            })
            .ok_or_else(|| {
                Error::new(
                    "expired_context",
                    "The context is unavailable. Read its source again.",
                )
            })?;
        for permission in &context.required {
            grant.require(*permission)?;
        }
        let allowed = context.entry_id.as_deref().map_or_else(
            || grant.scope.allows_url(&context.url),
            |id| {
                grant
                    .scope
                    .allows_entry(&self.instance_id, id, &context.url)
            },
        );
        if !allowed {
            return Err(denied("The context is outside this key's current scope."));
        }
        if context.entry_id.as_ref().is_some_and(|id| {
            self.application
                .copy_message_url(id, "client-request")
                .is_err()
        }) || context.source["kind"] == "capture"
            && context.source["captureId"]
                .as_str()
                .is_none_or(|id| self.application.trace_metadata(id).is_err())
        {
            return Err(Error::new(
                "source_unloaded",
                "This context source is no longer loaded.",
            ));
        }
        let offset = input["offset"]
            .as_str()
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| invalid("offset must be a nonnegative decimal byte position."))?;
        if offset > context.bytes.len() {
            return Err(invalid("offset exceeds the context length."));
        }
        let mut end = offset
            .saturating_add(index(input, "maxBytes", 16384, 65536)?)
            .min(context.bytes.len());
        let boundary = |position: usize| {
            context
                .bytes
                .get(position)
                .is_none_or(|byte| byte & 0xc0 != 0x80)
        };
        if context.text && boundary(offset) {
            while end > offset && !boundary(end) {
                end -= 1;
            }
            if end == offset {
                end = offset
                    .saturating_add(index(input, "maxBytes", 16384, 65536)?)
                    .min(context.bytes.len());
            }
        }
        let bytes = &context.bytes[offset..end];
        let (encoding, data) = if context.text {
            std::str::from_utf8(bytes).map_or_else(
                |_| ("base64", STANDARD.encode(bytes)),
                |text| ("utf-8", text.into()),
            )
        } else {
            ("base64", STANDARD.encode(bytes))
        };
        Ok(
            json!({"instanceId":self.instance_id,"contextId":context.id,"source":context.source,"metadata":context.metadata,"representation":context.representation,"encoding":encoding,"data":data,"offset":offset.to_string(),"totalBytes":context.bytes.len().to_string(),"nextOffset":(end<context.bytes.len()).then(||end.to_string()),"expiresAt":context.expires_at}),
        )
    }

    pub(super) fn context_json(
        &self,
        grant: &Grant,
        value: &Value,
        required: Vec<Permission>,
        source: Value,
        url: String,
        entry: Option<String>,
    ) -> Result<Value, Error> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| invalid("The complete context could not be encoded."))?;
        self.retain_context(
            grant,
            ContextInput {
                bytes,
                required,
                source,
                metadata: Value::Null,
                url,
                entry_id: entry,
                representation: "json",
                text: true,
            },
        )
    }

    pub(super) fn complete_headers(
        &self,
        grant: &Grant,
        id: &str,
        boundary: &str,
    ) -> Result<Value, Error> {
        let fields = self
            .application
            .message_headers(id, boundary)
            .map_err(app_error)?;
        Ok(authoring::test_headers(&fields, grant))
    }

    pub(super) fn headers_page(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.entry(grant, input, "id")?;
        let id = string(input, "id")?;
        let boundary = string(input, "boundary")?;
        let headers = self.complete_headers(grant, id, boundary)?;
        let url = self
            .application
            .copy_message_url(id, "client-request")
            .map_err(app_error)?;
        let required = if grant.allows(Permission::ReadCredentials) {
            vec![Permission::Inspect, Permission::ReadCredentials]
        } else {
            vec![Permission::Inspect]
        };
        let context = self.context_json(
            grant,
            &headers,
            required,
            json!({"kind":"headers","id":id,"boundary":boundary}),
            url,
            Some(id.into()),
        )?;
        let headers = headers.as_array().expect("header array");
        let offset = index(input, "offset", 0, usize::MAX as u64)?;
        let limit = index(input, "limit", 50, 200)?;
        let mut rows = Vec::new();
        let mut bytes = 0;
        for field in headers.iter().skip(offset).take(limit) {
            let size = serde_json::to_vec(field).map_or(0, |bytes| bytes.len());
            if bytes + size > 512 * 1024 {
                break;
            }
            bytes += size;
            rows.push(field.clone());
        }
        let next = offset.saturating_add(rows.len());
        Ok(
            json!({"headers":rows,"totalHeaders":headers.len(),"offset":offset,"nextOffset":(next<headers.len() && next>offset).then_some(next),"context":context,"coverage":"Inline fields contain complete values. The context retains the complete ordered header block."}),
        )
    }
}
