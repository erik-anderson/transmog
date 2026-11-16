//! Global, source-free configuration and startup health. Callback output stays exchange scoped.
use super::*;

impl AgentApplication {
    pub(super) fn scripts_status(&self, grant: &Grant, input: &Value) -> Result<Value, Error> {
        self.check_instance(input)?;
        grant.require(Permission::Inspect)?;
        if !matches!(grant.scope, Scope::All) {
            return Err(denied(
                "Script registry and initializer diagnostics require all-traffic scope. Inspect an authorized exchange with include changes for scoped callback evidence.",
            ));
        }
        let status = self.application.script_inspection_status();
        let offset = index(input, "offset", 0, usize::MAX as u64)?;
        let limit = index(input, "limit", 20, 100)?;
        let reveal = can_read_script_reports(grant);
        let include_diagnostics = input["includeDiagnostics"].as_bool().unwrap_or(false);
        let rows = status.scripts.iter().skip(offset).take(limit).map(|script| {
            let mut row = json!({"scriptId":script.script_id,"revision":script.revision,"savedRevision":script.saved_revision,"state":script.state,"available":script.available,"handlers":script.handlers,"contentAvailable":reveal});
            if reveal {
                row["prefilter"] = json!(script.prefilter);
                row["requestBodyFilter"] = json!(script.request_body_filter);
                row["responseBodyFilter"] = json!(script.response_body_filter);
                if include_diagnostics {
                    row["logs"] = json!(script.logs);
                    row["error"] = json!(script.error);
                    row["exception"] = json!(script.exception);
                }
            }
            row
        }).collect::<Vec<_>>();
        let next = offset.saturating_add(rows.len());
        let mut result = json!({"instanceId":self.instance_id,"generation":status.generation,"scripts":rows,"total":status.scripts.len(),"nextOffset":(next < status.scripts.len()).then_some(next),"coverage":"Current main-workspace configuration and initialized worker health. Draft admission settings may be unvalidated. Initializer output requires includeDiagnostics and unredacted access. Callback output and historical execution outcomes belong to scoped exchanges; script source and debugger commands are unavailable."});
        if serde_json::to_vec(&result).is_ok_and(|bytes| bytes.len() > 512 * 1024) {
            let mut required = vec![Permission::Inspect];
            if reveal {
                required.extend([Permission::ReadBodies, Permission::ReadCredentials]);
            }
            let context = self.context_json(
                grant,
                &result,
                required,
                json!({"kind":"script-status"}),
                "http://localhost/".into(),
                None,
            )?;
            result = json!({"instanceId":self.instance_id,"generation":status.generation,"total":status.scripts.len(),"nextOffset":result["nextOffset"],"context":context,"inlineComplete":false});
        }
        inspection::bounded(result, 512 * 1024)
    }
}
