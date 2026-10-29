//! Bounded, scoped inspection workflows and key-owned query snapshots.
use super::*;
use std::collections::HashSet;
use transmog_app::{
    FilterOperator, SessionColumnFilter, SessionSort, SortDirection, StreamQuery,
    TrafficSearchMode, TrafficSearchRequest,
};

pub(super) struct Snapshot {
    token: String,
    grant_id: String,
    revision: u64,
    expires_at: u64,
    ids: Vec<String>,
    required: Vec<Permission>,
    deadline: std::time::Instant,
    coverage: Option<Value>,
}

impl AgentApplication {
    fn scoped_ids(&self, grant: &Grant, input: &Value) -> Result<Vec<String>, Error> {
        grant.require(Permission::Inspect)?;
        let filters = &input["query"];
        let mut query = SessionQueryInput {
            search: optional_string(filters, "search")?,
            method: optional_string(filters, "method")?,
            host: optional_string(filters, "host")?,
            terminal: filters["terminal"].as_bool(),
            sort: Some(SessionSort {
                column: TrafficColumn::RequestNumber,
                direction: if filters["sort"] == "oldest" {
                    SortDirection::Ascending
                } else {
                    SortDirection::Descending
                },
            }),
            ..Default::default()
        };
        for (name, column, operator) in [
            ("statusMin", TrafficColumn::Status, FilterOperator::Minimum),
            ("statusMax", TrafficColumn::Status, FilterOperator::Maximum),
            (
                "durationMinMs",
                TrafficColumn::Duration,
                FilterOperator::Minimum,
            ),
            (
                "durationMaxMs",
                TrafficColumn::Duration,
                FilterOperator::Maximum,
            ),
            (
                "contentType",
                TrafficColumn::ContentType,
                FilterOperator::Contains,
            ),
            ("process", TrafficColumn::Process, FilterOperator::Contains),
            (
                "startedAfter",
                TrafficColumn::StartedAt,
                FilterOperator::Minimum,
            ),
            (
                "startedBefore",
                TrafficColumn::StartedAt,
                FilterOperator::Maximum,
            ),
        ] {
            if let Some(value) = filters.get(name) {
                query.filters.push(SessionColumnFilter {
                    column,
                    operator,
                    value: value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned),
                });
            }
        }
        validate_ranges(filters)?;
        if let Scope::Hosts { hosts } = &grant.scope {
            query.filter_groups.push(TrafficFilterGroup {
                dimension: TrafficDimension::Column(TrafficColumn::Host),
                values: hosts
                    .iter()
                    .cloned()
                    .map(TrafficFilterValue::Text)
                    .collect(),
            });
        }
        let mut ids = self
            .application
            .matching_traffic_ids(&query)
            .map_err(app_error)?;
        if let Scope::Requests {
            instance_id,
            ids: allowed,
        } = &grant.scope
        {
            if instance_id != &self.instance_id {
                return Err(Error::new(
                    "stale_instance",
                    "Reconnect with a selected-request scope for this workspace run.",
                ));
            }
            let allowed: HashSet<_> = allowed.iter().collect();
            ids.retain(|id| allowed.contains(id));
        }
        if ids.len() > 200_000 {
            return Err(Error::new(
                "limit",
                "Narrow the query to at most 200000 retained requests.",
            ));
        }
        Ok(ids)
    }

    fn save_snapshot(
        &self,
        grant: &Grant,
        ids: Vec<String>,
        required: Vec<Permission>,
        coverage: Option<Value>,
    ) -> Result<String, Error> {
        let token = random(18)?;
        let mut runtime = self.lock();
        runtime
            .snapshots
            .retain(|snapshot| snapshot.deadline > std::time::Instant::now());
        // A busy key cannot displace another key's active snapshots.
        if runtime
            .snapshots
            .iter()
            .filter(|snapshot| snapshot.grant_id == grant.id)
            .count()
            >= 8
            && let Some(index) = runtime
                .snapshots
                .iter()
                .position(|snapshot| snapshot.grant_id == grant.id)
        {
            runtime.snapshots.remove(index);
        }
        if runtime.snapshots.len() >= 128
            || runtime
                .snapshots
                .iter()
                .map(|snapshot| snapshot.ids.len())
                .sum::<usize>()
                .saturating_add(ids.len())
                > 500_000
        {
            return Err(Error::new(
                "limit",
                "Too many active traffic snapshots. Retry after a snapshot expires.",
            ));
        }
        runtime.snapshots.push_back(Snapshot {
            token: token.clone(),
            grant_id: grant.id.clone(),
            revision: grant.revision,
            expires_at: now() + 300,
            ids,
            required,
            deadline: std::time::Instant::now() + Duration::from_secs(300),
            coverage,
        });
        Ok(format!("{token}:0"))
    }

    fn read_snapshot(&self, grant: &Grant, cursor: &str, limit: usize) -> Result<Value, Error> {
        grant.require(Permission::Inspect)?;
        let (token, position) = cursor
            .split_once(':')
            .ok_or_else(|| invalid("cursor has an invalid format."))?;
        let position: usize = position
            .parse()
            .map_err(|_| invalid("cursor has an invalid position."))?;
        let (ids, total, expires, coverage, next) = {
            let runtime = self.lock();
            let snapshot = runtime
                .snapshots
                .iter()
                .find(|snapshot| {
                    snapshot.token == token
                        && snapshot.grant_id == grant.id
                        && snapshot.revision == grant.revision
                        && snapshot.deadline > std::time::Instant::now()
                })
                .ok_or_else(|| {
                    Error::new(
                        "expired_cursor",
                        "This query snapshot is unavailable. Start a new query.",
                    )
                })?;
            for permission in &snapshot.required {
                grant.require(*permission)?;
            }
            if position > snapshot.ids.len() {
                return Err(invalid("cursor position is outside the snapshot."));
            }
            let end = position.saturating_add(limit).min(snapshot.ids.len());
            (
                snapshot.ids[position..end].to_vec(),
                snapshot.ids.len(),
                snapshot.expires_at,
                snapshot.coverage.clone(),
                (end < snapshot.ids.len()).then(|| format!("{token}:{end}")),
            )
        };
        let rows = self
            .application
            .session_summaries(&ids)
            .map_err(app_error)?;
        let available: HashSet<_> = rows.iter().map(|row| row.id.as_str()).collect();
        let unavailable: Vec<_> = ids
            .iter()
            .filter(|id| !available.contains(id.as_str()))
            .collect();
        let rows: Vec<_> = rows.into_iter().map(|row| json!({"id":row.id,"requestNumber":row.request_number,"method":row.method,"url":row.url,"host":row.host,"status":row.status,"durationMs":row.duration_ms,"contentType":row.content_type,"startedAt":row.started_at,"terminal":row.terminal,"caller":row.caller,"streamProtocol":row.stream_protocol,"requestBytes":row.request_bytes,"responseBytes":row.response_bytes,"loss":row.loss})).collect();
        let mut result = json!({"instanceId":self.instance_id,"sessions":rows,"totalMatched":total,"nextCursor":next,"unavailableIds":unavailable,"expiresAt":expires,"coverage":"Snapshot membership and order; summary values reflect current retained evidence."});
        if let Some(coverage) = coverage {
            result["searchCoverage"] = coverage;
        }
        bounded(result, 512 * 1024)
    }

    pub(super) fn traffic_page(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        if input.get("instanceId").is_some() {
            self.check_instance(input)?;
        }
        let limit = index(input, "limit", 50, 200)?;
        let cursor = if let Some(cursor) = input["cursor"].as_str() {
            if input.get("query").is_some() {
                return Err(invalid("Continue with cursor and limit only."));
            }
            cursor.to_owned()
        } else {
            self.save_snapshot(grant, self.scoped_ids(grant, input)?, Vec::new(), None)?
        };
        self.read_snapshot(grant, &cursor, limit)
    }

    pub(super) fn exchange(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        let mut detail = self.detail(grant, input, "id")?;
        hide_bodies(&mut detail);
        let sections: Vec<_> = input["include"].as_array().map_or_else(
            || vec!["headers", "bodies"],
            |items| items.iter().filter_map(Value::as_str).collect(),
        );
        let mut result = json!({"instanceId":self.instance_id,"id":detail["id"],"requestNumber":detail["requestNumber"],"startedAt":detail["startedAt"],"terminal":detail["terminal"],"terminalState":detail["terminalState"]});
        if sections.contains(&"headers") {
            result["requests"] = detail["requests"].take();
            result["responses"] = detail["responses"].take();
        }
        if sections.contains(&"bodies") {
            result["bodies"] = detail["storedBodies"].take();
        }
        if sections.contains(&"timing") {
            result["timing"] = json!({"points":detail["performance"]["points"],"work":detail["performance"]["work"],"protocols":detail["performance"]["protocols"]});
        }
        if sections.contains(&"changes") {
            result["changes"] = json!({"hooks":detail["hookEffects"],"headers":detail["headerEffects"],"bodies":detail["bodyEdits"],"autoResponse":detail["autoResponse"]});
        }
        if sections.contains(&"transport") {
            result["transport"] = json!({"connections":detail["performance"]["transports"],"routeSelection":detail["routeSelection"],"routeAttempts":detail["routeAttempts"],"diagnostics":detail["diagnostics"],"stream":detail["stream"],"sequenceLoss":detail["sequenceLoss"]});
        }
        bounded(result, 512 * 1024)
    }

    pub(super) fn exchanges(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        let mut items = Vec::new();
        let mut remaining = 512 * 1024 - 20 * 4096 - 512;
        for id in input["ids"]
            .as_array()
            .ok_or_else(|| invalid("ids must be an array."))?
        {
            let call = json!({"instanceId":self.instance_id,"id":id,"include":input.get("include").cloned().unwrap_or_else(||json!(["headers","bodies"]))});
            let item = match self
                .exchange(grant, &call)
                .and_then(|item| bounded(item, remaining))
            {
                Ok(item) => item,
                Err(mut error) => {
                    error.message = error.message.chars().take(256).collect();
                    json!({"id":id,"error":error})
                }
            };
            remaining =
                remaining.saturating_sub(serde_json::to_vec(&item).map_or(0, |bytes| bytes.len()));
            items.push(item);
        }
        bounded(
            json!({"instanceId":self.instance_id,"items":items}),
            512 * 1024,
        )
    }

    pub(super) async fn search_traffic(
        &self,
        grant: &Grant,
        input: &Value,
    ) -> Result<Value, Error> {
        self.check_instance(input)?;
        let fields: Vec<_> = input["fields"].as_array().map_or_else(
            || vec!["metadata"],
            |values| values.iter().filter_map(Value::as_str).collect(),
        );
        if fields.is_empty() {
            return Err(invalid("fields must select at least one search source."));
        }
        let sensitive_search = fields.iter().any(|field| field.ends_with("bodies"));
        if sensitive_search {
            grant.require(Permission::ReadBodies)?;
        }
        let ids = self.scoped_ids(grant, input)?;
        let request = TrafficSearchRequest {
            operation_id: random(18)?,
            pattern: string(input, "pattern")?.into(),
            mode: if input["mode"] == "regex" {
                TrafficSearchMode::Regex
            } else {
                TrafficSearchMode::Text
            },
            case_sensitive: input["caseSensitive"].as_bool().unwrap_or(false),
            ignore_diacritics: false,
            metadata: fields.contains(&"metadata"),
            request_headers: fields.contains(&"request-headers"),
            response_headers: fields.contains(&"response-headers"),
            request_bodies: fields.contains(&"request-bodies"),
            response_bodies: fields.contains(&"response-bodies"),
        };
        let omit: fn(&str) -> bool = if grant.allows(Permission::ReadCredentials) {
            |_| false
        } else {
            sensitive
        };
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            self.application
                .search_traffic_scoped(request, ids.clone(), omit),
        )
        .await
        .map_err(|_| {
            Error::new(
                "timeout",
                "The scoped search exceeded 30 seconds. Narrow the query before retrying.",
            )
        })?
        .map_err(app_error)?;
        let matched: HashSet<_> = result.ids.iter().collect();
        let ids = ids.into_iter().filter(|id| matched.contains(id)).collect();
        let coverage = json!({"examined":result.examined,"binaryBodies":result.binary_bodies,"unavailableBodies":result.unavailable_bodies});
        let mut required = Vec::new();
        if sensitive_search {
            required.push(Permission::ReadBodies);
        }
        if grant.allows(Permission::ReadCredentials)
            && fields.iter().any(|field| field.ends_with("headers"))
        {
            required.push(Permission::ReadCredentials);
        }
        let cursor = self.save_snapshot(grant, ids, required, Some(coverage))?;
        self.read_snapshot(grant, &cursor, index(input, "limit", 50, 200)?)
    }

    pub(super) async fn wait_traffic(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        grant.require(Permission::Inspect)?;
        let after: u64 = string(input, "afterRequestNumber")?
            .parse()
            .map_err(|_| invalid("afterRequestNumber must fit an unsigned 64-bit number."))?;
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(integer(input, "timeoutMs", 30000, 30000)?);
        let mut updates = self
            .application
            .subscribe_session_updates()
            .map_err(app_error)?;
        loop {
            let mut ids = Vec::new();
            for chunk in self.scoped_ids(grant, input)?.chunks(200) {
                let rows = self
                    .application
                    .session_summaries(chunk)
                    .map_err(app_error)?;
                ids.extend(
                    rows.into_iter()
                        .filter(|row| {
                            row.request_number
                                .parse::<u64>()
                                .is_ok_and(|number| number > after)
                        })
                        .map(|row| row.id),
                );
            }
            let timed_out = tokio::time::Instant::now() >= deadline;
            if !ids.is_empty() || timed_out {
                let cursor = self.save_snapshot(grant, ids, Vec::new(), None)?;
                let mut result =
                    self.read_snapshot(grant, &cursor, index(input, "limit", 50, 200)?)?;
                result["timedOut"] = json!(timed_out && result["totalMatched"] == 0);
                return Ok(result);
            }
            tokio::select! { () = tokio::time::sleep_until(deadline)=>{}, result=updates.recv()=>{result.map_err(app_error)?;} }
        }
    }

    pub(super) async fn extract_body(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.entry(grant, input, "id")?;
        grant.require(Permission::ReadBodies)?;
        let id = string(input, "id")?;
        let boundary = string(input, "boundary")?;
        let body = self
            .application
            .inspect_body(BodyInspectionRequest {
                session_id: id.into(),
                boundary: boundary.into(),
                representation: BodyRepresentation::OriginalText,
                decode_content: true,
                offset: 0,
                max_bytes: Some(65536),
            })
            .await
            .map_err(app_error)?;
        if body.truncated
            || value(&body.metadata)?["availability"] != "complete"
            || body.representation != "original-text"
        {
            return Err(Error::new(
                "incomplete_body",
                "JSON extraction requires a complete decoded text body of at most 65536 bytes.",
            ));
        }
        let document: Value = serde_json::from_str(&body.display)
            .map_err(|_| invalid("The complete decoded body is not valid JSON."))?;
        let mut values = Vec::new();
        let invalid_escape = regex::Regex::new("~([^01]|$)").expect("constant regex");
        for pointer in input["pointers"]
            .as_array()
            .ok_or_else(|| invalid("pointers must be an array."))?
        {
            let pointer = pointer
                .as_str()
                .ok_or_else(|| invalid("A JSON pointer must be text."))?;
            if (!pointer.is_empty() && !pointer.starts_with('/'))
                || invalid_escape.is_match(pointer)
            {
                return Err(invalid("pointers must use RFC 6901 JSON Pointer syntax."));
            }
            values.push(document.pointer(pointer).map_or_else(
                || json!({"pointer":pointer,"found":false}),
                |value| json!({"pointer":pointer,"found":true,"value":value}),
            ));
        }
        bounded(
            json!({"instanceId":self.instance_id,"id":id,"boundary":boundary,"values":values,"coverage":"Complete decoded retained JSON document."}),
            256 * 1024,
        )
    }

    pub(super) fn prepare_request(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        let detail = self.entry(grant, input, "id")?;
        let id = string(input, "id")?;
        let boundary = input["boundary"].as_str().unwrap_or("client-request");
        let (mut request, omitted_headers) = self
            .application
            .captured_request_head(id, boundary)
            .map_err(app_error)?;
        if !grant.scope.allows_url(&request.url) {
            return Err(denied(
                "The replay destination is outside this key's destination scope.",
            ));
        }
        let include_credentials = input["includeCredentials"].as_bool().unwrap_or(false);
        if include_credentials {
            grant.require(Permission::ReadCredentials)?;
        }
        let nominated: HashSet<_> = request
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("connection"))
            .flat_map(|header| {
                header
                    .value
                    .split(',')
                    .map(|name| name.trim().to_ascii_lowercase())
            })
            .collect();
        let mut dropped:Vec<_> = omitted_headers.into_iter().map(|name|json!({"name":name,"reason":"The source capture omitted this header's value."})).collect();
        request.headers.retain(|header| {
            let name = header.name.to_ascii_lowercase();
            let reason = if [
                "host",
                "content-length",
                "transfer-encoding",
                "connection",
                "proxy-connection",
                "proxy-authorization",
                "proxy-authenticate",
                "keep-alive",
                "trailer",
                "te",
                "upgrade",
            ]
            .contains(&name.as_str())
                || nominated.contains(&name)
            {
                Some("Connection-specific fields are omitted; the send supplies required framing.")
            } else if sensitive(&name) && !include_credentials {
                Some("Credential header omitted; includeCredentials requires unredacted access.")
            } else {
                None
            };
            if let Some(reason) = reason {
                dropped.push(json!({"name":header.name,"reason":reason}));
                false
            } else {
                true
            }
        });
        let mut requirements = Vec::new();
        if !["GET", "HEAD", "OPTIONS", "TRACE", "PUT", "DELETE"].contains(&request.method.as_str())
        {
            requirements.push("Set acknowledgeNonIdempotent before sending this method.");
        }
        if request.headers.iter().any(|header| sensitive(&header.name)) {
            requirements.push("Set acknowledgeCredentials before sending captured credentials.");
        }
        if input["includeBody"].as_bool().unwrap_or(false) {
            grant.require(Permission::ReadBodies)?;
            if boundary != "client-request" {
                return Err(invalid(
                    "includeBody uses the captured client-request boundary. Prepare that boundary or supply an explicit replacement body.",
                ));
            }
            let complete = detail.stored_bodies.iter().any(|metadata| {
                metadata.boundary == "client-request"
                    && value(metadata).is_ok_and(|metadata| metadata["availability"] == "complete")
            });
            if !complete {
                return Err(Error::new(
                    "incomplete_body",
                    "The complete captured client-request body is unavailable. Supply an explicit replacement.",
                ));
            }
            request.body_source = Some(ComposerBodySource::Captured {
                entry_id: id.into(),
            });
        } else {
            requirements.push(
                "The body is omitted. Supply or explicitly include the captured body if needed.",
            );
        }
        bounded(
            json!({"instanceId":self.instance_id,"request":request,"droppedHeaders":dropped,"requirements":requirements}),
            512 * 1024,
        )
    }

    pub(super) fn stream_list(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.entry(grant, input, "id")?;
        let offset = index(input, "offset", 0, usize::MAX as u64)?;
        let page = self
            .application
            .stream_page(&StreamQuery {
                session_id: string(input, "id")?.into(),
                offset: index(input, "offset", 0, usize::MAX as u64)?,
                limit: index(input, "limit", 50, 200)?,
                tail: false,
                anchor: None,
                sender: optional_string(input, "sender")?,
                controls: input["controls"].as_bool().unwrap_or(false),
                search: String::new(),
            })
            .map_err(app_error)?;
        let rows: Vec<_> = page.rows.into_iter().filter(|_|offset<page.total).map(|row|json!({"sequence":row.sequence,"atMillis":row.message.at_millis,"sender":row.message.sender,"kind":row.message.kind,"observedBytes":row.message.byte_count,"truncated":row.message.truncated,"compressed":row.message.compressed})).collect();
        let next = offset.saturating_add(rows.len());
        Ok(
            json!({"instanceId":self.instance_id,"id":input["id"],"offset":offset,"total":page.total,"nextOffset":(next<page.total).then_some(next),"rows":rows}),
        )
    }

    pub(super) fn stream_read(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.entry(grant, input, "id")?;
        grant.require(Permission::ReadBodies)?;
        let message = self
            .application
            .stream_message(
                string(input, "id")?,
                integer(input, "sequence", 0, u64::MAX)?,
            )
            .map_err(app_error)?;
        let offset = index(input, "offset", 0, usize::MAX as u64)?;
        if offset > message.payload.len() {
            return Err(invalid("offset exceeds retained payload length."));
        }
        let end = offset
            .saturating_add(index(input, "maxBytes", 16384, 65536)?)
            .min(message.payload.len());
        Ok(
            json!({"instanceId":self.instance_id,"id":input["id"],"sequence":input["sequence"],"encoding":"base64","data":STANDARD.encode(&message.payload[offset..end]),"offset":offset,"retainedBytes":message.payload.len(),"observedBytes":message.byte_count,"truncated":message.truncated,"availability":if message.kind=="unavailable" || message.payload.is_empty() && message.byte_count>0 {"unavailable"} else if message.truncated || (message.payload.len() as u64)<message.byte_count {"truncated"} else {"complete"},"nextOffset":(end<message.payload.len()).then_some(end)}),
        )
    }
}

fn bounded(value: Value, max: usize) -> Result<Value, Error> {
    if serde_json::to_vec(&value)
        .map_err(|_| invalid("Result encoding failed."))?
        .len()
        > max
    {
        return Err(Error::new(
            "limit",
            "The result exceeds its byte budget. Request fewer items or evidence sections.",
        ));
    }
    Ok(value)
}

fn validate_ranges(filters: &Value) -> Result<(), Error> {
    for (min, max) in [
        ("statusMin", "statusMax"),
        ("durationMinMs", "durationMaxMs"),
        ("startedAfter", "startedBefore"),
    ] {
        if filters[min]
            .as_u64()
            .zip(filters[max].as_u64())
            .is_some_and(|(left, right)| left > right)
        {
            return Err(invalid(&format!(
                "query.{min} must not exceed query.{max}."
            )));
        }
    }

    Ok(())
}
