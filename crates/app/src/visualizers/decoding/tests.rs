use super::*;
use crate::{
    AppConfig, Application, BodyStoreConfig,
    body_store::SavedBodySource,
    visualizers::{
        boundaries,
        evidence::project_input,
        source_infos,
        tests::{declaration, snapshot},
    },
};
use std::{
    io::{self, Cursor, Read},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use transmog_core::{intercept::ExchangeId, observe::ExchangeBoundary};
use transmog_extension_registry::InstanceResources;
use transmog_extensions::ReadRequest;

#[derive(Debug)]
struct Blob {
    bytes: Arc<[u8]>,
    opened: Arc<AtomicUsize>,
}
impl SavedBodySource for Blob {
    fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        self.open_at(0)
    }
    fn open_at(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        self.opened.fetch_add(1, Ordering::Relaxed);
        let mut reader = Cursor::new(Arc::clone(&self.bytes));
        reader.set_position(offset);
        Ok(Box::new(reader))
    }
    fn length(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }
}
struct Fixture {
    app: Application,
    resources: Arc<Resources>,
    id: String,
    opened: Arc<AtomicUsize>,
    _root: tempfile::TempDir,
}
fn fixture(bytes: Vec<u8>, codings: &[&str]) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let app = Application::new(AppConfig {
        body_store: Some(BodyStoreConfig::product_default(root.path().join("body"))),
        ..AppConfig::default()
    })
    .unwrap();
    let mut snapshot = snapshot(&app);
    snapshot.request_heads[0].head.headers =
        HeaderBlock::from_fields(vec![HeaderField::try_new("X-Kind", "same").unwrap()]);
    let mut headers = HeaderBlock::default();
    if !codings.is_empty() {
        headers.push(HeaderField::try_new("Content-Encoding", codings.join(", ")).unwrap());
    }
    snapshot.response_heads[0].head.headers = headers.clone();
    let opened = Arc::new(AtomicUsize::new(0));
    let length = bytes.len() as u64;
    app.body_store.as_ref().unwrap().register_saved(
        ExchangeId(1),
        ExchangeBoundary::ClientResponse,
        &headers,
        None,
        Arc::new(Blob {
            bytes: Arc::from(bytes),
            opened: Arc::clone(&opened),
        }),
        length,
        true,
    );
    let mut declaration = declaration();
    declaration.inputs.response.body = BodyAccess::Optional;
    declaration
        .inputs
        .response
        .headers
        .push("content-encoding".into());
    declaration.inputs.response.decode_http_content = true;
    let selected = boundaries("client-response").unwrap();
    let mut sources = BTreeMap::new();
    let input = project_input(
        &snapshot,
        selected,
        &declaration,
        app.body_store.as_ref(),
        "decoding-worker",
        &mut sources,
    )
    .unwrap();
    let id = input
        .response
        .as_ref()
        .unwrap()
        .body
        .as_ref()
        .unwrap()
        .source_id
        .clone();
    let resources = Arc::new(Resources {
        live_available: false,
        live: Mutex::new(None),
        stopped: AtomicBool::new(false),
        sources: Mutex::new(
            sources
                .into_iter()
                .map(|(id, source)| (id, Arc::new(source)))
                .collect(),
        ),
        info: Mutex::new(source_infos(&snapshot, selected, &input)),
        context: Mutex::new(None),
    });
    Fixture {
        app,
        resources,
        id,
        opened,
        _root: root,
    }
}
fn ready(result: HttpDecodeResult) -> DecodedBody {
    let HttpDecodeResult::Ready { decoded } = result else {
        panic!("decoded source required")
    };
    decoded
}

#[tokio::test]
async fn http_sources_are_lazy_separate_bounded_and_revoked_on_unload() {
    for codings in [
        vec!["gzip"],
        vec!["deflate"],
        vec!["br"],
        vec!["zstd"],
        vec!["gzip", "br"],
    ] {
        let labels = codings
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let encoded =
            crate::inspector::encode_content(&labels, b"synthetic decoded bytes".to_vec())
                .await
                .unwrap();
        let case = fixture(encoded.clone(), &codings);
        assert_eq!(case.opened.load(Ordering::Relaxed), 0);
        let decoded = ready(
            case.resources
                .decode_http(&case.id, 1024 * 1024)
                .await
                .unwrap(),
        );
        assert_ne!(decoded.body.source_id, case.id);
        assert_eq!(decoded.parent.source_id, case.id);
        assert_eq!(decoded.parent.length, encoded.len().to_string());
        assert_eq!(decoded.codings, labels);
        let (_, page) = case
            .resources
            .read(&ReadRequest {
                source_id: decoded.body.source_id.clone(),
                offset: "0".into(),
                max_bytes: 9,
            })
            .unwrap();
        assert_eq!(page.bytes, b"synthetic");
        let opens = case.opened.load(Ordering::Relaxed);
        assert_eq!(
            ready(
                case.resources
                    .decode_http(&case.id, 1024 * 1024)
                    .await
                    .unwrap()
            ),
            decoded
        );
        assert_eq!(
            case.opened.load(Ordering::Relaxed),
            opens,
            "A repeated decode must reuse its exact immutable representation"
        );
        let (_, original) = case
            .resources
            .read(&ReadRequest {
                source_id: case.id.clone(),
                offset: "0".into(),
                max_bytes: encoded.len().min(8),
            })
            .unwrap();
        assert_eq!(original.bytes, encoded[..encoded.len().min(8)]);
        assert!(
            case.resources
                .info()
                .unwrap()
                .iter()
                .any(|info| info.http_decoded.as_ref() == Some(&decoded))
        );
        case.resources.stop();
        assert!(
            case.resources
                .read(&ReadRequest {
                    source_id: decoded.body.source_id,
                    offset: "0".into(),
                    max_bytes: 1
                })
                .is_err()
        );
        assert!(case.resources.decode_http(&case.id, 1024).await.is_err());
    }
}
#[tokio::test]
async fn invalid_unavailable_and_over_budget_decodes_never_substitute_encoded_bytes() {
    let malformed = fixture(vec![1, 2, 3, 4], &["gzip"]);
    assert_eq!(
        malformed
            .resources
            .decode_http(&malformed.id, 1024)
            .await
            .unwrap(),
        HttpDecodeResult::Unavailable {
            reason: DecodeFailure::Malformed
        }
    );
    assert!(malformed.opened.load(Ordering::Relaxed) > 0);
    assert!(
        malformed
            .resources
            .info()
            .unwrap()
            .iter()
            .all(|info| info.http_decoded.is_none())
    );
    let unsupported = fixture(vec![1, 2, 3], &["unknown"]);
    assert_eq!(
        unsupported
            .resources
            .decode_http(&unsupported.id, 1024)
            .await
            .unwrap(),
        HttpDecodeResult::Unavailable {
            reason: DecodeFailure::Unsupported
        }
    );
    assert_eq!(unsupported.opened.load(Ordering::Relaxed), 0);
    let encoded = crate::inspector::encode_content(&["gzip".into()], vec![b'A'; 4096])
        .await
        .unwrap();
    let bomb = fixture(encoded, &["gzip"]);
    assert_eq!(
        bomb.resources.decode_http(&bomb.id, 128).await.unwrap(),
        HttpDecodeResult::Unavailable {
            reason: DecodeFailure::Limited
        }
    );
    assert!(
        bomb.opened.load(Ordering::Relaxed) > 0,
        "The codec expansion limit must be exercised after the encoded source is read"
    );
    assert!(
        bomb.resources
            .info()
            .unwrap()
            .iter()
            .all(|info| info.http_decoded.is_none())
    );
    let identity = fixture(b"raw".to_vec(), &[]);
    assert_eq!(
        ready(
            identity
                .resources
                .decode_http(&identity.id, 1024)
                .await
                .unwrap()
        )
        .body
        .source_id,
        identity.id
    );
    assert_eq!(identity.opened.load(Ordering::Relaxed), 0);
}
#[test]
fn missing_or_redacted_dependencies_never_authorize_hidden_coding_metadata() {
    let mut access = declaration().inputs.response;
    access.decode_http_content = true;
    access.body = BodyAccess::Optional;
    assert!(grant(&HeaderBlock::default(), &access).is_none());
    access.headers.push("content-encoding".into());
    let mut redacted = HeaderField::try_new("Content-Encoding", "gzip").unwrap();
    redacted.redact_value();
    assert_eq!(
        grant(&HeaderBlock::from_fields(vec![redacted]), &access),
        Some(Err(DecodeFailure::MetadataUnavailable))
    );
    access.body = BodyAccess::None;
    assert!(grant(&HeaderBlock::default(), &access).is_none());
}

#[tokio::test]
async fn decoding_requires_complete_consistent_retention_and_revokes_cached_bytes() {
    let encoded = crate::inspector::encode_content(&["gzip".into()], b"retained".to_vec())
        .await
        .unwrap();
    for (availability, codings, reason) in [
        (
            crate::BodyAvailability::Truncated,
            vec!["gzip".into()],
            DecodeFailure::Incomplete,
        ),
        (
            crate::BodyAvailability::Complete,
            vec!["br".into()],
            DecodeFailure::MetadataUnavailable,
        ),
    ] {
        let case = fixture(encoded.clone(), &["gzip"]);
        {
            let mut sources = case.resources.sources.lock().unwrap();
            let source = Arc::get_mut(sources.get_mut(&case.id).unwrap()).unwrap();
            let Source::Body { metadata, .. } = source else {
                panic!("original body required")
            };
            metadata.availability = availability;
            metadata.content_codings = codings;
        }
        assert_eq!(
            case.resources.decode_http(&case.id, 1024).await.unwrap(),
            HttpDecodeResult::Unavailable { reason }
        );
        assert_eq!(case.opened.load(Ordering::Relaxed), 0);
    }
    let case = fixture(encoded, &["gzip"]);
    let decoded = ready(case.resources.decode_http(&case.id, 1024).await.unwrap());
    case.app
        .body_store
        .as_ref()
        .unwrap()
        .forget_entries(&[ExchangeId(1)])
        .unwrap();
    assert!(
        case.resources
            .read(&ReadRequest {
                source_id: decoded.body.source_id,
                offset: "0".into(),
                max_bytes: 1
            })
            .is_err()
    );
    assert!(case.resources.decode_http(&case.id, 1024).await.is_err());
}
