use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use transmog_automation::{AutomationResponse, ResponseAssetResolver};
use transmog_core::{
    BodyFrame, BodyStream, BodyStreamError, CanonicalResponse, HeaderBlock, HeaderField,
    HttpLegVersion, LocalStreamingResponse, ResponseHead, intercept::ExchangeId,
    observe::ExchangeBoundary,
};
use transmog_script::{ScriptResolvedResponse, ScriptResponseAssetResolver};

use crate::{AppError, BodyAvailability, BodyStore, ErrorCategory};

const ASSET_SCHEMA_VERSION: u32 = 1;
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_AUTHORED_BYTES: usize = 16 * 1024 * 1024;
const BUFFERED_RESPONSE_BYTES: u64 = 1024 * 1024;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Origin of a complete reusable response asset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResponseAssetProvenance {
    /// Body and metadata were authored explicitly.
    Authored,
    /// Body was copied from a complete retained traffic boundary.
    Session {
        /// Source exchange identifier.
        exchange_id: String,
        /// Source body boundary.
        boundary: String,
    },
    /// Body was imported from an explicit local file.
    Imported,
}

/// Complete immutable response asset metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseAsset {
    /// Stable asset identifier.
    pub id: String,
    /// Monotonic asset revision.
    pub revision: u64,
    /// Final HTTP response status.
    pub status: u16,
    /// Ordered headers after hop-by-hop and stale entity metadata repair.
    pub headers: HeaderBlock,
    /// Exact body byte length.
    pub body_bytes: u64,
    /// SHA-256 of the exact stored body.
    pub sha256: String,
    /// Declared media type when available.
    pub media_type: Option<String>,
    /// Creation provenance.
    pub provenance: ResponseAssetProvenance,
}

impl ResponseAsset {
    /// Stable exact revision reference accepted by automation rules and scripts.
    pub fn asset_ref(&self) -> String {
        format!("{}@{}", self.id, self.revision)
    }
}

/// Authored small response input.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthoredResponseAsset {
    /// Stable asset identifier.
    pub id: String,
    /// New immutable revision.
    pub revision: u64,
    /// HTTP response status.
    pub status: u16,
    /// Ordered response headers.
    pub headers: HeaderBlock,
    /// Bounded exact body bytes.
    pub body: Vec<u8>,
    /// Optional declared media type.
    pub media_type: Option<String>,
}

/// Explicit file import for a potentially large streaming response asset.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportResponseAsset {
    /// Stable asset identifier.
    pub id: String,
    /// New immutable revision.
    pub revision: u64,
    /// HTTP response status.
    pub status: u16,
    /// Ordered response headers.
    pub headers: HeaderBlock,
    /// Explicit body file selected by the caller.
    pub body_path: PathBuf,
    /// Optional declared media type.
    pub media_type: Option<String>,
}

/// Complete retained traffic boundary converted into a response asset.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionResponseAsset {
    /// Stable asset identifier.
    pub id: String,
    /// New immutable revision.
    pub revision: u64,
    /// Source exchange identifier as 32 hexadecimal digits.
    pub exchange_id: String,
    /// Response boundary, currently `client-response`.
    pub boundary: String,
    /// Optional decoded replacement body edited by the user.
    #[serde(default)]
    pub decoded_body: Option<Vec<u8>>,
    /// Reapply the source response's content-coding stack after an edit.
    #[serde(default = "default_true")]
    pub preserve_content_encoding: bool,
}

/// Bounded, decoded saved-response preview for the properties editor.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseAssetInspection {
    /// Immutable response metadata and ordered headers.
    pub asset: ResponseAsset,
    /// Complete decoded text, when it can be edited faithfully.
    pub display: String,
    /// Character encoding used when saving text edits.
    pub text_encoding: Option<&'static str>,
    /// Original compression stack.
    pub content_codings: Vec<String>,
    /// Why a preview is read-only or unavailable.
    pub explanation: String,
}

/// A new immutable revision of a saved response.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseAssetEdit {
    /// Exact source revision; its bytes and historical metadata remain intact.
    pub asset_reference: String,
    /// New response status.
    pub status: u16,
    /// Ordered editable response fields.
    pub headers: HeaderBlock,
    /// Optional declared media type.
    pub media_type: Option<String>,
    /// Replacement decoded bytes, or null to retain the original encoded body.
    pub decoded_body: Option<Vec<u8>>,
    /// Explicitly selected replacement body file, served with identity encoding.
    #[serde(default)]
    pub body_path: Option<PathBuf>,
    /// Reapply the original compression stack to replacement bytes.
    #[serde(default = "default_true")]
    pub preserve_content_encoding: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssetIndex {
    schema_version: u32,
    generation: u64,
    assets: Vec<ResponseAsset>,
}

/// Durable content-addressed response asset store.
#[derive(Clone, Debug)]
pub struct ResponseAssetStore {
    mutation: Arc<Mutex<()>>,
    root: Option<Arc<PathBuf>>,
    assets: Arc<RwLock<BTreeMap<String, ResponseAsset>>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
}

impl ResponseAssetStore {
    pub(crate) fn load(root: Option<PathBuf>) -> Result<Self, AppError> {
        let (generation, assets) = if let Some(root) = root.as_deref() {
            std::fs::create_dir_all(root)
                .map_err(|_| unavailable("response asset directory is unavailable"))?;
            load_index(root)?
        } else {
            (0, BTreeMap::new())
        };
        Ok(Self {
            mutation: Arc::new(Mutex::new(())),
            root: root.map(Arc::new),
            assets: Arc::new(RwLock::new(assets)),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(generation)),
        })
    }

    /// Lists immutable assets in stable ID/revision order.
    pub fn list(&self) -> Vec<ResponseAsset> {
        self.assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    fn get(&self, reference: &str) -> Result<ResponseAsset, AppError> {
        self.assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(reference)
            .cloned()
            .ok_or_else(|| invalid("Saved response revision is unavailable."))
    }

    fn open_body(&self, asset: &ResponseAsset) -> Result<File, AppError> {
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| unavailable("Saved response storage is unavailable."))?;
        let file = File::open(body_path(root, &asset.sha256))
            .map_err(|_| unavailable("Saved response body is unavailable."))?;
        if file
            .metadata()
            .map_err(|_| unavailable("Saved response body is unreadable."))?
            .len()
            != asset.body_bytes
        {
            return Err(unavailable("Saved response body length changed."));
        }
        Ok(file)
    }

    pub(crate) async fn inspect(
        &self,
        reference: &str,
    ) -> Result<ResponseAssetInspection, AppError> {
        let asset = self.get(reference)?;
        let content_codings = asset_codings(&asset.headers);
        let mut result=ResponseAssetInspection {asset:asset.clone(),display:String::new(),text_encoding:None,content_codings:content_codings.clone(),explanation:"Binary or unsupported text. Headers and status remain editable; replace the body from a file if needed.".into()};
        if asset.body_bytes > MAX_AUTHORED_BYTES as u64 {
            result.explanation =
                "Body exceeds the 16 MiB text editor limit. Status and headers remain editable."
                    .into();
            return Ok(result);
        }
        let mut bytes = Vec::new();
        self.open_body(&asset)?
            .take(MAX_AUTHORED_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable("Saved response body is unreadable."))?;
        if bytes.len() as u64 != asset.body_bytes {
            return Err(unavailable("Saved response body changed."));
        }
        let bytes = crate::inspector::decode_content(&content_codings, bytes).await?;
        let charset = asset
            .media_type
            .as_deref()
            .and_then(|mime| {
                mime.split(';')
                    .skip(1)
                    .find_map(|part| part.trim().strip_prefix("charset="))
            })
            .map(|value| value.trim_matches('"'));
        let (_, display, warning, encoding) = crate::inspector::render_body(
            crate::BodyRepresentation::OriginalText,
            &bytes,
            charset,
            asset.media_type.as_deref(),
            0,
        );
        result.display = display;
        result.text_encoding = encoding;
        result.explanation=warning.unwrap_or_else(||"Edits create a new saved response revision. Historical traffic retains its original response.".into());
        Ok(result)
    }

    pub(crate) async fn edit(&self, input: ResponseAssetEdit) -> Result<ResponseAsset, AppError> {
        let source = self.get(&input.asset_reference)?;
        let codings = asset_codings(&source.headers);
        let mut headers = input.headers;
        headers.remove_all("content-encoding");
        if input.body_path.is_some() && input.decoded_body.is_some() {
            return Err(invalid("Choose either a replacement file or edited text."));
        }
        let replacement_file = if let Some(path) = &input.body_path {
            let file =
                File::open(path).map_err(|_| invalid("Replacement body file is unavailable."))?;
            if !file
                .metadata()
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_ASSET_BYTES)
            {
                return Err(invalid("Replacement file exceeds one GiB."));
            }
            Some(file)
        } else {
            None
        };
        let encoded = if let Some(decoded) = input.decoded_body {
            if decoded.len() > MAX_AUTHORED_BYTES {
                return Err(invalid("Edited body exceeds 16 MiB."));
            }
            if input.preserve_content_encoding {
                Some(crate::inspector::encode_content(&codings, decoded).await?)
            } else {
                Some(decoded)
            }
        } else {
            None
        };
        if replacement_file.is_none() && (encoded.is_none() || input.preserve_content_encoding) {
            for field in source
                .headers
                .iter()
                .filter(|field| field.name().eq_ignore_ascii_case(b"content-encoding"))
            {
                headers.push(field.clone());
            }
        }
        headers.remove_all("content-type");
        let revision = self
            .list()
            .iter()
            .filter(|asset| asset.id == source.id)
            .map(|asset| asset.revision)
            .max()
            .unwrap_or(source.revision)
            .checked_add(1)
            .ok_or_else(|| invalid("Saved response revision is exhausted."))?;
        if let Some(file) = replacement_file {
            return self.create_from_reader(
                source.id,
                revision,
                input.status,
                &headers,
                input.media_type,
                source.provenance,
                file,
            );
        }
        if let Some(body) = encoded {
            self.create_from_reader(
                source.id,
                revision,
                input.status,
                &headers,
                input.media_type,
                source.provenance,
                std::io::Cursor::new(body),
            )
        } else {
            let reader = self.open_body(&source)?;
            self.create_from_reader(
                source.id,
                revision,
                input.status,
                &headers,
                input.media_type,
                source.provenance,
                reader,
            )
        }
    }

    pub(crate) fn discard_created(&self, references: &[String]) -> Result<(), AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for reference in references {
            next.remove(reference);
        }
        let generation = self
            .generation
            .load(std::sync::atomic::Ordering::Acquire)
            .checked_add(1)
            .ok_or_else(|| invalid("Asset generation exhausted."))?;
        persist_index(
            self.root
                .as_deref()
                .ok_or_else(|| unavailable("Asset storage unavailable."))?,
            generation,
            &next,
        )?;
        *self
            .assets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
        self.generation
            .store(generation, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    /// Creates a small authored asset without overwriting an existing revision.
    ///
    /// # Errors
    /// Returns validation, collision, size, or persistence failures.
    pub fn create_authored(&self, input: AuthoredResponseAsset) -> Result<ResponseAsset, AppError> {
        if input.body.len() > MAX_AUTHORED_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "authored response body exceeds the sixteen MiB limit; import a file instead",
                false,
            ));
        }
        self.create_from_reader(
            input.id,
            input.revision,
            input.status,
            &input.headers,
            input.media_type,
            ResponseAssetProvenance::Authored,
            std::io::Cursor::new(input.body),
        )
    }

    /// Imports a potentially large file with streaming hashing and storage.
    ///
    /// # Errors
    /// Returns validation, collision, file, quota, or persistence failures.
    pub fn import_file(&self, input: ImportResponseAsset) -> Result<ResponseAsset, AppError> {
        let metadata = std::fs::metadata(&input.body_path)
            .map_err(|_| invalid("response asset import file is unavailable"))?;
        if !metadata.is_file() || metadata.len() > MAX_ASSET_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "response asset import exceeds the one GiB limit",
                false,
            ));
        }
        let file = File::open(&input.body_path)
            .map_err(|_| invalid("response asset import file is unreadable"))?;
        self.create_from_reader(
            input.id,
            input.revision,
            input.status,
            &input.headers,
            input.media_type,
            ResponseAssetProvenance::Imported,
            file,
        )
    }

    /// Copies one exact complete retained body under an eviction lease.
    ///
    /// # Errors
    /// Incomplete, truncated, lossy, evicted, or disabled boundaries fail.
    pub fn create_from_session(
        &self,
        body_store: &BodyStore,
        input: SessionResponseAsset,
        status: u16,
        headers: &HeaderBlock,
        media_type: Option<String>,
        encoded_body_override: Option<Vec<u8>>,
    ) -> Result<ResponseAsset, AppError> {
        let exchange_id = parse_exchange_id(&input.exchange_id)?;
        let boundary = parse_response_boundary(&input.boundary)?;
        let metadata = body_store
            .metadata(exchange_id)
            .into_iter()
            .find(|candidate| candidate.boundary == input.boundary)
            .ok_or_else(|| invalid("response asset requires a retained client response"))?;
        if metadata.availability != BodyAvailability::Complete {
            return Err(invalid("response asset requires a complete retained body"));
        }
        if metadata.retained_bytes > MAX_ASSET_BYTES {
            return Err(AppError::new(
                ErrorCategory::Limit,
                "response asset exceeds the one GiB limit",
                false,
            ));
        }
        let media_type = media_type.or_else(|| metadata.media_type.clone());
        let provenance = ResponseAssetProvenance::Session {
            exchange_id: input.exchange_id,
            boundary: input.boundary,
        };
        match encoded_body_override {
            Some(body) => self.create_from_reader(
                input.id,
                input.revision,
                status,
                headers,
                media_type,
                provenance,
                std::io::Cursor::new(body),
            ),
            None if metadata.retained_bytes == 0 => self.create_from_reader(
                input.id,
                input.revision,
                status,
                headers,
                media_type,
                provenance,
                std::io::Cursor::new(Vec::<u8>::new()),
            ),
            None => {
                let lease = body_store
                    .open_complete(exchange_id, boundary)
                    .map_err(|_| invalid("response asset requires a complete retained body"))?;
                self.create_from_reader(
                    input.id,
                    input.revision,
                    status,
                    headers,
                    media_type,
                    provenance,
                    lease,
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_from_reader(
        &self,
        id: String,
        revision: u64,
        status: u16,
        headers: &HeaderBlock,
        media_type: Option<String>,
        provenance: ResponseAssetProvenance,
        reader: impl Read,
    ) -> Result<ResponseAsset, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_identity(&id, revision)?;
        if headers.iter().count() > 256
            || headers
                .iter()
                .map(|field| field.name().len() + field.value().len())
                .sum::<usize>()
                > 64 * 1024
        {
            return Err(invalid("Response headers exceed the editor limits."));
        }
        if !(200..=599).contains(&status) {
            return Err(invalid("response asset status must be between 200 and 599"));
        }
        let key = format!("{id}@{revision}");
        if self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&key)
        {
            return Err(AppError::new(
                ErrorCategory::Conflict,
                "response asset revision already exists",
                false,
            ));
        }
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| unavailable("response asset persistence is not configured"))?;
        let (body_bytes, sha256) = store_body(root, reader)?;
        let headers = repair_headers(headers, body_bytes, media_type.as_deref())?;
        let asset = ResponseAsset {
            id,
            revision,
            status,
            headers,
            body_bytes,
            sha256,
            media_type,
            provenance,
        };
        let mut next = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        next.insert(key, asset.clone());
        let generation = self
            .generation
            .load(std::sync::atomic::Ordering::Acquire)
            .checked_add(1)
            .ok_or_else(|| {
                AppError::new(ErrorCategory::Limit, "asset generation exhausted", false)
            })?;
        persist_index(root, generation, &next)?;
        *self
            .assets
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
        self.generation
            .store(generation, std::sync::atomic::Ordering::Release);
        Ok(asset)
    }
}

fn asset_codings(headers: &HeaderBlock) -> Vec<String> {
    headers
        .values("content-encoding")
        .filter_map(|value| std::str::from_utf8(value).ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value != "identity" && !value.is_empty())
        .collect()
}

impl ResponseAssetResolver for ResponseAssetStore {
    fn validate(&self, asset_id: &str) -> Result<(), String> {
        let asset = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(asset_id)
            .cloned()
            .ok_or_else(|| "response asset revision is unavailable".to_owned())?;
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| "response asset persistence is unavailable".to_owned())?;
        if std::fs::metadata(body_path(root, &asset.sha256))
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == asset.body_bytes)
        {
            Ok(())
        } else {
            Err("response asset body is unavailable".to_owned())
        }
    }

    fn resolve(&self, asset_id: &str) -> Result<AutomationResponse, String> {
        let asset = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(asset_id)
            .cloned()
            .ok_or_else(|| "response asset revision is unavailable".to_owned())?;
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| "response asset persistence is unavailable".to_owned())?;
        let path = body_path(root, &asset.sha256);
        let metadata = std::fs::metadata(&path)
            .map_err(|_| "response asset body is unavailable".to_owned())?;
        if metadata.len() != asset.body_bytes {
            return Err("response asset body length no longer matches metadata".to_owned());
        }
        let head = ResponseHead {
            status: asset.status,
            headers: asset.headers.clone(),
            source_version: HttpLegVersion::Http1,
        };
        if asset.body_bytes <= BUFFERED_RESPONSE_BYTES {
            let body = std::fs::read(path)
                .map_err(|_| "response asset body could not be read".to_owned())?;
            return Ok(AutomationResponse::Buffered(CanonicalResponse::local(
                head.status,
                head.headers,
                Bytes::from(body),
            )));
        }
        let (sender, body) =
            BodyStream::channel(NonZeroUsize::new(8).expect("asset stream capacity is nonzero"));
        let expected = asset.body_bytes;
        std::thread::Builder::new()
            .name("transmog-response-asset".to_owned())
            .spawn(move || stream_file(path, expected, &sender))
            .map_err(|_| "response asset stream could not start".to_owned())?;
        Ok(AutomationResponse::Streaming(LocalStreamingResponse {
            head,
            body,
            body_length: Some(asset.body_bytes),
        }))
    }
}

impl ScriptResponseAssetResolver for ResponseAssetStore {
    fn validate(&self, asset_ref: &str) -> Result<(), String> {
        ResponseAssetResolver::validate(self, asset_ref)
    }

    fn resolve(&self, asset_ref: &str) -> Result<ScriptResolvedResponse, String> {
        match ResponseAssetResolver::resolve(self, asset_ref)? {
            AutomationResponse::Buffered(response) => {
                Ok(ScriptResolvedResponse::Buffered(response))
            }
            AutomationResponse::Streaming(response) => {
                Ok(ScriptResolvedResponse::Streaming(response))
            }
        }
    }
}

fn stream_file(path: PathBuf, expected: u64, sender: &transmog_core::BodyStreamSender) {
    let result = (|| -> Result<(), BodyStreamError> {
        let mut file = File::open(path)
            .map_err(|_| BodyStreamError::Failed("response asset body open failed".to_owned()))?;
        let mut total = 0_u64;
        let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
        loop {
            let read = file.read(&mut buffer).map_err(|_| {
                BodyStreamError::Failed("response asset body read failed".to_owned())
            })?;
            if read == 0 {
                break;
            }
            total = total.checked_add(read as u64).ok_or_else(|| {
                BodyStreamError::Failed("response asset length overflow".to_owned())
            })?;
            if total > expected {
                return Err(BodyStreamError::Failed(
                    "response asset body grew while streaming".to_owned(),
                ));
            }
            sender
                .blocking_send(Ok(BodyFrame::Data(Bytes::copy_from_slice(&buffer[..read]))))
                .map_err(|_| BodyStreamError::Failed("response consumer closed".to_owned()))?;
        }
        if total != expected {
            return Err(BodyStreamError::Failed(
                "response asset body changed while streaming".to_owned(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = sender.blocking_send(Err(error));
    }
}

fn validate_identity(id: &str, revision: u64) -> Result<(), AppError> {
    if id.is_empty()
        || id.len() > 128
        || revision == 0
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(invalid("response asset identity is invalid"));
    }
    Ok(())
}

fn repair_headers(
    headers: &HeaderBlock,
    body_bytes: u64,
    media_type: Option<&str>,
) -> Result<HeaderBlock, AppError> {
    let nominated = headers
        .values("connection")
        .flat_map(|value| value.split(|byte| *byte == b','))
        .filter_map(|value| std::str::from_utf8(value).ok())
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut repaired = HeaderBlock::new();
    for field in headers.iter() {
        HeaderField::try_new(field.name().to_vec(), field.value().to_vec())
            .map_err(|_| invalid("response asset contains an invalid header"))?;
        let name = String::from_utf8_lossy(field.name()).to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-connection"
                | "transfer-encoding"
                | "upgrade"
                | "te"
                | "trailer"
                | "content-length"
                | "content-md5"
                | "digest"
        ) || nominated.iter().any(|nominated| nominated == &name)
        {
            continue;
        }
        repaired.push(field.clone());
    }
    if repaired.values("content-type").next().is_none()
        && let Some(media_type) = media_type
    {
        repaired.push(
            HeaderField::try_new("content-type", media_type)
                .map_err(|_| invalid("response asset media type is invalid"))?,
        );
    }
    repaired.push(
        HeaderField::try_new("content-length", body_bytes.to_string())
            .expect("decimal content length is a valid field"),
    );
    Ok(repaired)
}

fn store_body(root: &Path, mut reader: impl Read) -> Result<(u64, String), AppError> {
    let temp = root.join(format!(".asset-tmp-{}", random_hex()?));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|_| unavailable("response asset temporary file is unavailable"))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
    let result = (|| -> Result<(), AppError> {
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|_| unavailable("response asset source read failed"))?;
            if read == 0 {
                break;
            }
            total = total.checked_add(read as u64).ok_or_else(|| {
                AppError::new(ErrorCategory::Limit, "response asset is too large", false)
            })?;
            if total > MAX_ASSET_BYTES {
                return Err(AppError::new(
                    ErrorCategory::Limit,
                    "response asset exceeds the one GiB limit",
                    false,
                ));
            }
            output
                .write_all(&buffer[..read])
                .map_err(|_| unavailable("response asset body write failed"))?;
            hasher.update(&buffer[..read]);
        }
        output
            .sync_all()
            .map_err(|_| unavailable("response asset body finalization failed"))
    })();
    if let Err(error) = result {
        drop(output);
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    drop(output);
    let digest = hasher.finalize();
    let sha256 = hex_digest(&digest);
    let destination = body_path(root, &sha256);
    if destination.exists() {
        let _ = std::fs::remove_file(&temp);
    } else {
        std::fs::rename(&temp, destination)
            .map_err(|_| unavailable("response asset body commit failed"))?;
    }
    Ok((total, sha256))
}

fn body_path(root: &Path, sha256: &str) -> PathBuf {
    root.join(format!("asset-{sha256}.body"))
}

fn index_path(root: &Path, slot: u64) -> PathBuf {
    root.join(format!("index.{slot}.json"))
}

fn persist_index(
    root: &Path,
    generation: u64,
    assets: &BTreeMap<String, ResponseAsset>,
) -> Result<(), AppError> {
    let document = AssetIndex {
        schema_version: ASSET_SCHEMA_VERSION,
        generation,
        assets: assets.values().cloned().collect(),
    };
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|_| unavailable("response asset index serialization failed"))?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(AppError::new(
            ErrorCategory::Limit,
            "response asset index exceeds its four MiB limit",
            false,
        ));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(index_path(root, generation % 2))
        .map_err(|_| unavailable("response asset index cannot be opened"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| unavailable("response asset index persistence failed"))
}

fn load_index(root: &Path) -> Result<(u64, BTreeMap<String, ResponseAsset>), AppError> {
    let mut documents = Vec::new();
    for slot in 0..=1 {
        let path = index_path(root, slot);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_INDEX_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(document) = serde_json::from_slice::<AssetIndex>(&bytes) else {
            continue;
        };
        if document.schema_version == ASSET_SCHEMA_VERSION {
            documents.push(document);
        }
    }
    documents.sort_by_key(|document| document.generation);
    let Some(document) = documents.pop() else {
        return Ok((0, BTreeMap::new()));
    };
    let mut assets = BTreeMap::new();
    for asset in document.assets {
        validate_identity(&asset.id, asset.revision)?;
        if std::fs::metadata(body_path(root, &asset.sha256))
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == asset.body_bytes)
        {
            assets.insert(asset.asset_ref(), asset);
        }
    }
    Ok((document.generation, assets))
}

pub(crate) fn parse_exchange_id(id: &str) -> Result<ExchangeId, AppError> {
    if id.len() != 32 {
        return Err(invalid("session identifier is invalid"));
    }
    u128::from_str_radix(id, 16)
        .map(ExchangeId)
        .map_err(|_| invalid("session identifier is invalid"))
}

pub(crate) fn parse_response_boundary(value: &str) -> Result<ExchangeBoundary, AppError> {
    match value {
        "client-response" => Ok(ExchangeBoundary::ClientResponse),
        _ => Err(invalid(
            "response asset source must be the client-visible response",
        )),
    }
}

pub(crate) fn random_hex() -> Result<String, AppError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| unavailable("response asset temporary identity failed"))?;
    Ok(hex_digest(&bytes))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut output, byte| {
            let _ = std::fmt::Write::write_fmt(&mut output, format_args!("{byte:02x}"));
            output
        },
    )
}

fn invalid(message: &'static str) -> AppError {
    AppError::new(ErrorCategory::InvalidInput, message, false)
}

fn unavailable(message: &'static str) -> AppError {
    AppError::new(ErrorCategory::Unavailable, message, true)
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, num::NonZeroUsize, sync::Arc};

    use transmog_core::{
        ConnectionId, RequestHead, SessionId, SessionMetadata, StreamId, Target,
        intercept::{CompletedExchange, ExchangeMetadata},
        observe::{Observer, ObserverEvent, ObserverEventKind},
    };

    use crate::{BodyStoreConfig, RetentionMode};

    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("transmog-assets-{}", random_hex().unwrap()))
    }

    #[test]
    fn authored_asset_repairs_metadata_and_resolves_exact_revision() {
        let root = root();
        let store = ResponseAssetStore::load(Some(root.clone())).unwrap();
        let asset = store
            .create_authored(AuthoredResponseAsset {
                id: "saved".to_owned(),
                revision: 3,
                status: 200,
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("connection", "close").unwrap(),
                    HeaderField::try_new("content-length", "999").unwrap(),
                ]),
                body: b"exact".to_vec(),
                media_type: Some("text/plain; charset=utf-8".to_owned()),
            })
            .unwrap();
        assert_eq!(asset.asset_ref(), "saved@3");
        assert!(asset.headers.values("connection").next().is_none());
        assert_eq!(
            asset.headers.values("content-length").next(),
            Some(&b"5"[..])
        );
        assert!(matches!(
            ResponseAssetResolver::resolve(&store, "saved@3").unwrap(),
            AutomationResponse::Buffered(_)
        ));
        assert!(ResponseAssetResolver::resolve(&store, "saved@2").is_err());
        drop(store);
        assert_eq!(
            ResponseAssetStore::load(Some(root.clone())).unwrap().list(),
            [asset]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn large_file_asset_resolves_to_a_backpressured_stream() {
        let root = root();
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("large.input");
        std::fs::write(
            &source,
            vec![0x5a; usize::try_from(BUFFERED_RESPONSE_BYTES).unwrap() + 1],
        )
        .unwrap();
        let store = ResponseAssetStore::load(Some(root.clone())).unwrap();
        let asset = store
            .import_file(ImportResponseAsset {
                id: "large".to_owned(),
                revision: 1,
                status: 200,
                headers: HeaderBlock::new(),
                body_path: source,
                media_type: None,
            })
            .unwrap();
        assert_eq!(asset.body_bytes, BUFFERED_RESPONSE_BYTES + 1);
        assert!(matches!(
            ResponseAssetResolver::resolve(&store, "large@1").unwrap(),
            AutomationResponse::Streaming(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn empty_client_response_can_be_cloned_with_protected_headers() {
        let root = root();
        let body_root = root.join("bodies");
        let body_store = BodyStore::new(BodyStoreConfig {
            root: body_root,
            mode: RetentionMode::Circular,
            max_bytes: 1024,
            max_body_bytes: 1024,
            queue_capacity: NonZeroUsize::new(8).unwrap(),
            max_read_bytes: 1024,
            retain_requests: false,
        })
        .unwrap();
        let target = Target {
            scheme: "https".to_owned(),
            authority: "example.test".to_owned(),
            host: "example.test".to_owned(),
            port: 443,
            path: "/empty".to_owned(),
            query: None,
        };
        let session = SessionMetadata {
            session_id: SessionId(17),
            downstream_connection_id: ConnectionId(17),
            stream_id: StreamId(17),
            client_addr: "127.0.0.1:1000".parse::<SocketAddr>().unwrap(),
            client_identity: transmog_core::ClientIdentity::default(),
            proxy_addr: "127.0.0.1:2000".parse::<SocketAddr>().unwrap(),
            ingress_version: HttpLegVersion::Http1,
            egress_version: None,
        };
        let metadata = Arc::new(ExchangeMetadata::from_session(&session, target.clone()));
        let response = ResponseHead {
            status: 204,
            source_version: HttpLegVersion::Http1,
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("set-cookie", "session=secret").unwrap(),
                HeaderField::try_new("set-cookie", "theme=dark").unwrap(),
                HeaderField::try_new("content-length", "0").unwrap(),
            ]),
        };
        for event in [
            ObserverEvent {
                exchange_id: ExchangeId(17),
                sequence: 1,
                kind: ObserverEventKind::ExchangeStarted {
                    metadata: Arc::clone(&metadata),
                },
            },
            ObserverEvent {
                exchange_id: ExchangeId(17),
                sequence: 2,
                kind: ObserverEventKind::ResponseHeadObserved {
                    boundary: ExchangeBoundary::ClientResponse,
                    head: response.clone(),
                },
            },
            ObserverEvent {
                exchange_id: ExchangeId(17),
                sequence: 3,
                kind: ObserverEventKind::Completed(CompletedExchange {
                    metadata,
                    request_head: RequestHead {
                        method: "GET".to_owned(),
                        target,
                        source_version: HttpLegVersion::Http1,
                        headers: HeaderBlock::new(),
                    },
                    response_head: response.clone(),
                }),
            },
        ] {
            Observer::on_event(&body_store, event).await.unwrap();
        }
        body_store.flush().unwrap();
        let store = ResponseAssetStore::load(Some(root.clone())).unwrap();
        let asset = store
            .create_from_session(
                &body_store,
                SessionResponseAsset {
                    id: "empty".to_owned(),
                    revision: 1,
                    exchange_id: format!("{:032x}", 17),
                    boundary: "client-response".to_owned(),
                    decoded_body: None,
                    preserve_content_encoding: true,
                },
                response.status,
                &response.headers,
                None,
                None,
            )
            .unwrap();
        assert_eq!(asset.body_bytes, 0);
        assert_eq!(
            asset.headers.values("set-cookie").collect::<Vec<_>>(),
            [&b"session=secret"[..], &b"theme=dark"[..]]
        );
        assert_eq!(
            asset.headers.values("content-length").next(),
            Some(&b"0"[..])
        );
        drop(body_store);
        let _ = std::fs::remove_dir_all(root);
    }
}
