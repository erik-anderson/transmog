//! Read-only access to recorded context, without exporting traffic bodies.
use crate::{arguments, invalid_input, output, passwords};
use serde::Serialize;
use serde_json::Value;
use std::{
    error::Error,
    fs::OpenOptions,
    io::{self, Read, Seek, Write},
    path::Path,
};
use transmog_capture::{
    CaptureError, CaptureLimits, CapturePassword, CaptureReader, CaptureRecordKind,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAX_ITEMS: usize = 256;
const MAX_CONTEXT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    kind: String,
    source_key: Option<String>,
    source_name: Option<String>,
    collected_at: Option<Value>,
    state: &'static str,
    completeness: &'static str,
    issues: Vec<String>,
    #[serde(skip)]
    content: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Inventory {
    version: u8,
    sealed: bool,
    truncated_tail: bool,
    body_integrity_checked: bool,
    contexts: Vec<Item>,
}

#[derive(Default)]
struct Options<'a> {
    input: Option<&'a str>,
    id: Option<&'a str>,
    output: Option<&'a str>,
    password: Option<&'a str>,
    password_file: Option<&'a str>,
    list: bool,
    no_prompt: bool,
}
impl<'a> Options<'a> {
    fn parse(arguments: &'a [String]) -> io::Result<Self> {
        arguments::validate(
            arguments,
            &[
                "--input",
                "--id",
                "--output",
                "--password",
                "--password-file",
            ],
            &["--list", "--no-prompt"],
        )?;
        let mut options = Self::default();
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--list" => options.list = true,
                "--no-prompt" => options.no_prompt = true,
                name => {
                    // Validation guarantees a value. Consume it even when it
                    // resembles an option: passwords are opaque strings.
                    let value = arguments.next().map(String::as_str);
                    match name {
                        "--input" => options.input = value,
                        "--id" => options.id = value,
                        "--output" => options.output = value,
                        "--password" => options.password = value,
                        "--password-file" => options.password_file = value,
                        _ => unreachable!("validated option"),
                    }
                }
            }
        }
        if options.password.is_some() && options.password_file.is_some() {
            return Err(invalid_input(
                "Choose --password or --password-file, not both",
            ));
        }
        if options.list && options.id.is_some() {
            return Err(invalid_input("Choose --list or --id, not both"));
        }
        if options
            .password
            .is_some_and(|text| text.is_empty() || text.len() > 4096)
        {
            return Err(invalid_input("Password must contain 1 to 4096 UTF-8 bytes"));
        }
        Ok(options)
    }
}

pub(crate) fn command(arguments: &[String]) -> Result<()> {
    let options = Options::parse(arguments)?;
    let path = options
        .input
        .ok_or_else(|| invalid_input("missing required --input"))?;
    let mut open_options = OpenOptions::new();
    open_options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open_options.share_mode(5); // Readers share the pinned file; replacement preserves this snapshot.
    }
    let mut file = open_options.open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(invalid_input("Context input must be a native capture file").into());
    }
    let inventory = unlock(
        &options,
        |password| {
            file.rewind()?;
            read(&mut file, password)
        },
        || passwords::prompt(false),
    )?;
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        return Err(invalid_input(
            "Capture changed while reading context; retry with a saved copy",
        )
        .into());
    }
    if !inventory.sealed || inventory.truncated_tail {
        eprintln!(
            "Reading context from an unsealed or incomplete committed capture prefix; later context may be unavailable."
        );
    }
    let selected = options
        .id
        .map(|id| {
            let item = inventory
                .contexts
                .iter()
                .find(|item| item.id == id)
                .ok_or_else(|| {
                    invalid_input("Context ID not found. Use --list with this input to find an ID")
                })?;
            if item.content.is_null() {
                return Err(invalid_input("The selected context was not collected"));
            }
            Ok(&item.content)
        })
        .transpose()?;
    let write = |output: &mut dyn Write| -> Result<()> {
        if let Some(content) = selected {
            serde_json::to_writer_pretty(&mut *output, content)?;
        } else {
            serde_json::to_writer_pretty(&mut *output, &inventory)?;
        }
        output.write_all(b"\n")?;
        Ok(())
    };
    match options.output.unwrap_or("-") {
        "-" => write(&mut io::stdout().lock()),
        path => output::write_new(Path::new(path), |file| {
            write(file)?;
            file.sync_all()?;
            Ok(())
        }),
    }
}

fn unlock<T>(
    options: &Options<'_>,
    mut read: impl FnMut(Option<&CapturePassword>) -> Result<T>,
    mut prompt: impl FnMut() -> io::Result<CapturePassword>,
) -> Result<T> {
    let mut password = options
        .password
        .map(|text| CapturePassword::new(text.to_owned()));
    if let Some(path) = options.password_file {
        password = Some(passwords::read_file(path)?);
    }
    let explicit = password.is_some();
    let no_prompt = options.no_prompt;
    loop {
        match read(password.as_ref()) {
            Ok(value) => return Ok(value),
            Err(error) => {
                let auth = matches!(
                    error.downcast_ref::<CaptureError>(),
                    Some(CaptureError::PasswordRequired | CaptureError::InvalidPassword)
                );
                if !auth || explicit {
                    return Err(error);
                }
                if no_prompt {
                    return Err(invalid_input("Encrypted input needs a password; --no-prompt disables prompting. Supply --password or --password-file").into());
                }
                if password.is_some() {
                    eprintln!("The capture password was not accepted.");
                }
                password = Some(prompt()?);
            }
        }
    }
}

fn read(input: impl Read + Seek, password: Option<&CapturePassword>) -> Result<Inventory> {
    let mut reader = CaptureReader::with_password(input, CaptureLimits::default(), password)?;
    let mut contexts = vec![];
    let mut bytes = 0;
    while let Some(frame) = reader.read_next_indexed()? {
        if frame.record.exchange_id != 0 {
            continue;
        }
        let CaptureRecordKind::Unknown { kind, payload } = frame.record.kind else {
            continue;
        };
        if matches!(kind.as_str(), "incident-marker" | "circular-retention") {
            append(
                &mut contexts,
                &mut bytes,
                &format!("record/{}/", frame.index),
                &kind,
                &payload,
                None,
            )?;
            continue;
        }
        let (context, source, prefix) = match kind.as_str() {
            "trace-metadata" => (&payload, None, format!("record/{}/", frame.index)),
            "trace-source" => (
                &payload["context"],
                Some(&payload),
                format!("record/{}/context/", frame.index),
            ),
            _ => continue,
        };
        if let Some(fields) = context.as_object() {
            for (kind, content) in fields {
                append(&mut contexts, &mut bytes, &prefix, kind, content, source)?;
            }
        } else if !context.is_null() {
            append(
                &mut contexts,
                &mut bytes,
                &prefix,
                "context",
                context,
                source,
            )?;
        }
        if let Some(source) = source
            && source["notes"]
                .as_array()
                .is_some_and(|notes| !notes.is_empty())
        {
            append(
                &mut contexts,
                &mut bytes,
                &format!("record/{}/", frame.index),
                "notes",
                &source["notes"],
                Some(source),
            )?;
        }
    }
    Ok(Inventory {
        version: 0,
        sealed: reader.sealed(),
        truncated_tail: reader.truncated_tail(),
        body_integrity_checked: false,
        contexts,
    })
}

fn append(
    contexts: &mut Vec<Item>,
    bytes: &mut usize,
    prefix: &str,
    kind: &str,
    content: &Value,
    source: Option<&Value>,
) -> Result<()> {
    if contexts.len() >= MAX_ITEMS || kind.len() > 512 {
        return Err(invalid_input(
            "Recorded context exceeds the 256-item or 512-byte field-name limit",
        )
        .into());
    }
    if source.is_some_and(|source| {
        source["id"].as_str().is_some_and(|id| id.len() > 512)
            || source["name"]
                .as_str()
                .is_some_and(|name| name.len() > 4096)
    }) {
        return Err(
            invalid_input("Recorded context source identity or label exceeds its bound").into(),
        );
    }
    *bytes = bytes
        .checked_add(serde_json::to_vec(content)?.len())
        .filter(|size| *size <= MAX_CONTEXT_BYTES)
        .ok_or_else(|| invalid_input("Recorded context exceeds 16 MiB"))?;
    let network = (kind == "networkContext")
        .then(|| {
            serde_json::from_value::<transmog_network::context::NetworkContext>(content.clone())
                .ok()
        })
        .flatten();
    let (completeness, issues) = network.map_or(("unknown", vec![]), |network| {
        let state = if network.output.is_empty() {
            "unavailable"
        } else if network.notes.is_empty() {
            "complete"
        } else {
            "partial"
        };
        (state, network.notes)
    });
    let source_text = |field: &str| source.and_then(|s| s[field].as_str()).map(str::to_owned);
    contexts.push(Item {
        id: format!("{prefix}{}", kind.replace('~', "~0").replace('/', "~1")),
        kind: kind.into(),
        source_key: source_text("id"),
        source_name: source_text("name"),
        collected_at: content.get("collectedAt").cloned(),
        state: if content.is_null() {
            "not-collected"
        } else {
            "recorded"
        },
        completeness,
        issues,
        content: content.clone(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        io::{Cursor, SeekFrom},
        rc::Rc,
    };
    use transmog_capture::{CaptureEncoding, CaptureRecord, CaptureWriter};
    use transmog_core::observe::ExchangeBoundary;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).into()).collect()
    }

    #[test]
    fn password_policy_prompts_only_when_needed_and_never_after_explicit_failure() {
        for arguments in [
            args(&["--no-prompt"]),
            args(&["--password", "wrong"]),
            args(&["--password", "wrong", "--no-prompt"]),
        ] {
            let mut calls = 0;
            let result = unlock(
                &Options::parse(&arguments).unwrap(),
                |_| {
                    calls += 1;
                    Err::<(), _>(CaptureError::PasswordRequired.into())
                },
                || panic!("must not prompt"),
            );
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
        assert_eq!(
            unlock(
                &Options::default(),
                |_| Ok(17),
                || panic!("plain input must not prompt")
            )
            .unwrap(),
            17
        );
        let mut prompts = 0;
        let value = unlock(
            &Options::default(),
            |password| match password {
                None => Err(CaptureError::PasswordRequired.into()),
                Some(value) if value.bytes() == b"correct" => Ok(23),
                Some(_) => Err(CaptureError::InvalidPassword.into()),
            },
            || {
                prompts += 1;
                Ok(CapturePassword::new(
                    if prompts == 1 { "wrong" } else { "correct" }.into(),
                ))
            },
        )
        .unwrap();
        assert_eq!(value, 23);
        assert_eq!(prompts, 2);
        let result = unlock(
            &Options::default(),
            |_| Err::<(), _>(CaptureError::InvalidMagic.into()),
            || panic!("corruption must not prompt"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn context_scan_skips_large_body_payloads_and_reports_committed_prefixes() {
        struct Counted {
            input: Cursor<Vec<u8>>,
            reads: Rc<Cell<usize>>,
        }
        impl Read for Counted {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                let n = self.input.read(output)?;
                self.reads.set(self.reads.get() + n);
                Ok(n)
            }
        }
        impl Seek for Counted {
            fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
                self.input.seek(position)
            }
        }
        let mut data = Cursor::new(vec![]);
        {
            let mut writer = CaptureWriter::with_encoding(
                &mut data,
                CaptureLimits::default(),
                &CaptureEncoding::default(),
            )
            .unwrap();
            writer
                .append(&CaptureRecord {
                    sequence: 0,
                    exchange_id: 0,
                    kind: CaptureRecordKind::Unknown {
                        kind: "trace-metadata".into(),
                        payload: serde_json::json!({"networkContext":null}),
                    },
                })
                .unwrap();
            writer.checkpoint().unwrap();
            let mut random = 0x1234_5678_u32;
            let bytes: Vec<u8> = (0..1024 * 1024)
                .map(|_| {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    random.to_le_bytes()[0]
                })
                .collect();
            writer
                .append(&CaptureRecord {
                    sequence: 1,
                    exchange_id: 1,
                    kind: CaptureRecordKind::BodySegment {
                        boundary: ExchangeBoundary::ClientRequest,
                        byte_count: bytes.len(),
                        bytes: Some(bytes),
                        truncated: false,
                    },
                })
                .unwrap();
            writer.seal().unwrap();
        }
        assert!(data.get_ref().len() > 1024 * 1024);
        data.rewind().unwrap();
        let reads = Rc::new(Cell::new(0));
        let result = read(
            Counted {
                input: data.clone(),
                reads: reads.clone(),
            },
            None,
        )
        .unwrap();
        assert!(result.sealed);
        assert!(!result.body_integrity_checked);
        assert_eq!(result.contexts[0].state, "not-collected");
        assert!(reads.get() < 256 * 1024, "read {} bytes", reads.get());
        let mut unsealed = Cursor::new(vec![]);
        let mut writer = CaptureWriter::new(&mut unsealed, CaptureLimits::default()).unwrap();
        writer
            .append(&CaptureRecord {
                sequence: 0,
                exchange_id: 0,
                kind: CaptureRecordKind::Unknown {
                    kind: "trace-metadata".into(),
                    payload: serde_json::json!({"purpose":"original"}),
                },
            })
            .unwrap();
        writer.checkpoint().unwrap();
        drop(writer);
        unsealed.get_mut().extend_from_slice(b"incomplete tail");
        unsealed.rewind().unwrap();
        let result = read(unsealed, None).unwrap();
        assert!(!result.sealed);
        assert!(result.truncated_tail);
        assert_eq!(result.contexts[0].content, "original");
    }
}
