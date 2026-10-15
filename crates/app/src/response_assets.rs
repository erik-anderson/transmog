use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use transmog_automation::{AutomationResponse, ResponseAssetResolver};
use transmog_content::{ContentCodingStack, ContentEncoder, ContentLimits};
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

/// Explicit selected-file behavior; size never changes this choice.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseBodyFileMode {
    /// Copy exact bytes into immutable response storage when saving.
    #[default]
    Copy,
    /// Read the selected external file for each matching request.
    Linked,
}

/// Complete ordered response headers for local auto-response authoring.
pub type ResponseAssetHeaders = HeaderBlock;

/// A response which reads the current external file for every request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkedResponseBody {
    /// Absolute external path, retained even when the file is unavailable.
    pub path: PathBuf,
    /// Compression stack from the response used as the replacement's source.
    #[serde(default)]
    pub original_content_codings: Vec<String>,
    /// Apply the retained compression stack to current file bytes.
    #[serde(default)]
    pub apply_original_compression: bool,
}

/// Readable selected-file metadata, independent of the chosen save behavior.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseBodyFileInfo {
    /// Absolute selected path.
    pub path: PathBuf,
    /// Current byte length before compression.
    pub bytes: u64,
    /// Last serving failure, retained until a subsequent response succeeds.
    pub error: Option<String>,
}

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
    /// Exact stored length, or the external file size when the link was saved.
    pub body_bytes: u64,
    /// SHA-256 of stored bytes; empty for a live external file.
    pub sha256: String,
    /// Declared media type when available.
    pub media_type: Option<String>,
    /// Creation provenance.
    pub provenance: ResponseAssetProvenance,
    /// External body configuration; absent for durable copied bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_body: Option<LinkedResponseBody>,
    /// Informational source path for copied bytes; never reopened when serving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied_from: Option<PathBuf>,
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
    /// Copy by default; linking is always an explicit caller choice.
    #[serde(default)]
    pub body_file_mode: ResponseBodyFileMode,
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
    /// Optional selected decoded file body, replacing the retained source bytes.
    #[serde(default)]
    pub body_path: Option<PathBuf>,
    /// How to save a selected file; ignored when no file is selected.
    #[serde(default)]
    pub body_file_mode: ResponseBodyFileMode,
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
    /// Complete decoded bytes for lossless Text/Hex editing, absent when unavailable.
    pub bytes_base64: Option<String>,
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
    /// How to save a selected file; ignored when no file is selected.
    #[serde(default)]
    pub body_file_mode: ResponseBodyFileMode,
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
    file_errors: Arc<Mutex<BTreeMap<String, String>>>,
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
            file_errors: Arc::new(Mutex::new(BTreeMap::new())),
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
        let mut result=ResponseAssetInspection {asset:asset.clone(),display:String::new(),text_encoding:None,bytes_base64:None,content_codings:content_codings.clone(),explanation:"Body unavailable. Headers and status remain editable; replace the body from a file if needed.".into()};
        if let Some(linked) = &asset.linked_body {
            result
                .content_codings
                .clone_from(&linked.original_content_codings);
            result.explanation="Response body is read directly from the linked file. Clear the file choice to author text, or choose another file.".into();
            return Ok(result);
        }
        if asset.body_bytes > MAX_AUTHORED_BYTES as u64 {
            let mut preview = Vec::new();
            self.open_body(&asset)?
                .take(4096)
                .read_to_end(&mut preview)
                .map_err(|_| unavailable("Saved response body is unreadable."))?;
            result.display = crate::inspector::render_body(
                crate::BodyRepresentation::Bytes,
                &preview,
                None,
                None,
                0,
            )
            .1;
            result.explanation = format!(
                "Read-only original-byte preview: first {} of {} bytes (before decoding compression). Body exceeds the 16 MiB editor limit. Serve the complete body from a file to replace it.",
                preview.len(),
                asset.body_bytes
            );
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
        let preview = bytes[..bytes.len().min(4096)].to_vec();
        let bytes = match crate::inspector::decode_content(&content_codings, bytes).await {
            Ok(bytes) => bytes,
            Err(error) => {
                result.display = crate::inspector::render_body(
                    crate::BodyRepresentation::Bytes,
                    &preview,
                    None,
                    None,
                    0,
                )
                .1;
                result.explanation = format!(
                    "Read-only original-byte preview: first {} of {} bytes (before decoding compression). Decoded body unavailable: {}. Serve the complete body from a file to replace it.",
                    preview.len(),
                    asset.body_bytes,
                    error.message
                );
                return Ok(result);
            }
        };
        let charset = asset.media_type.as_deref().and_then(|mime| {
            mime.split(';').skip(1).find_map(|part| {
                let (name, value) = part.split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("charset")
                    .then(|| value.trim().trim_matches('"').trim())
            })
        });
        let (_, display, warning, encoding) = crate::inspector::render_body(
            crate::BodyRepresentation::OriginalText,
            &bytes,
            charset,
            asset.media_type.as_deref(),
            0,
        );
        result.display = display;
        result.text_encoding = encoding;
        result.bytes_base64 = Some(STANDARD.encode(&bytes));
        result.explanation = if warning.is_some() {
            "Binary body · Hex. Edit hexadecimal byte pairs, or replace the body from a file."
                .into()
        } else {
            "Edits create a new saved response revision. Historical traffic retains its original response.".into()
        };
        Ok(result)
    }

    pub(crate) async fn edit(&self, input: ResponseAssetEdit) -> Result<ResponseAsset, AppError> {
        let source = self.get(&input.asset_reference)?;
        let codings = source.linked_body.as_ref().map_or_else(
            || asset_codings(&source.headers),
            |linked| linked.original_content_codings.clone(),
        );
        let mut headers = input.headers;
        headers.remove_all("content-encoding");
        if input.body_path.is_some() && input.decoded_body.is_some() {
            return Err(invalid("Choose either a replacement file or edited text."));
        }
        let replacement_file = input
            .body_path
            .as_deref()
            .map(open_selected_file)
            .transpose()?;
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
        if encoded.is_some() && input.preserve_content_encoding {
            set_content_encoding(&mut headers, &codings)?;
        } else if replacement_file.is_none() && encoded.is_none() {
            for field in source
                .headers
                .iter()
                .filter(|field| field.name().eq_ignore_ascii_case(b"content-encoding"))
            {
                headers.push(field.clone());
            }
        }
        headers.remove_all("content-type");
        let revision = self.next_revision(&source)?;
        if let Some((file, info)) = replacement_file {
            let mut replacement = source.clone();
            replacement.revision = revision;
            replacement.status = input.status;
            replacement.headers = headers;
            replacement.media_type = input.media_type;
            return self
                .create_file_revision(
                    replacement,
                    file,
                    info,
                    input.body_file_mode,
                    codings,
                    input.preserve_content_encoding,
                )
                .await;
        }
        if encoded.is_none()
            && let Some(mut linked) = source.linked_body.clone()
        {
            linked.apply_original_compression = input.preserve_content_encoding;
            return self.create_linked(
                source.id,
                revision,
                input.status,
                &headers,
                input.media_type,
                source.provenance,
                linked,
                source.body_bytes,
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
                None,
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
                source.copied_from,
                reader,
            )
        }
    }

    fn next_revision(&self, source: &ResponseAsset) -> Result<u64, AppError> {
        self.list()
            .iter()
            .filter(|asset| asset.id == source.id)
            .map(|asset| asset.revision)
            .max()
            .unwrap_or(source.revision)
            .checked_add(1)
            .ok_or_else(|| invalid("Saved response revision is exhausted."))
    }

    async fn create_file_revision(
        &self,
        asset: ResponseAsset,
        file: File,
        info: ResponseBodyFileInfo,
        mode: ResponseBodyFileMode,
        codings: Vec<String>,
        preserve: bool,
    ) -> Result<ResponseAsset, AppError> {
        if mode == ResponseBodyFileMode::Linked {
            return self.create_linked(
                asset.id,
                asset.revision,
                asset.status,
                &asset.headers,
                asset.media_type,
                asset.provenance,
                LinkedResponseBody {
                    path: info.path,
                    original_content_codings: codings,
                    apply_original_compression: preserve,
                },
                info.bytes,
            );
        }
        if preserve && !codings.is_empty() {
            let copied_from = info.path.clone();
            let root: PathBuf = self
                .root
                .as_deref()
                .ok_or_else(|| unavailable("Saved response storage is unavailable."))?
                .clone();
            let applied = codings.clone();
            let mut encoded = tokio::task::spawn_blocking(move || {
                encode_file_temporary(file, info.bytes, &applied, &root)
            })
            .await
            .map_err(|_| unavailable("Response file compression could not complete."))??;
            let mut headers = asset.headers;
            set_content_encoding(&mut headers, &codings)?;
            return self.create_from_reader(
                asset.id,
                asset.revision,
                asset.status,
                &headers,
                asset.media_type,
                asset.provenance,
                Some(copied_from),
                encoded.as_file_mut(),
            );
        }
        self.create_from_reader(
            asset.id,
            asset.revision,
            asset.status,
            &asset.headers,
            asset.media_type,
            asset.provenance,
            Some(info.path),
            ExactFileReader::new(file, info.bytes),
        )
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
            None,
            std::io::Cursor::new(input.body),
        )
    }

    /// Imports a potentially large file with streaming hashing and storage.
    ///
    /// # Errors
    /// Returns validation, collision, file, quota, or persistence failures.
    pub fn import_file(&self, input: ImportResponseAsset) -> Result<ResponseAsset, AppError> {
        let (file, info) = open_selected_file(&input.body_path)?;
        let mut headers = input.headers;
        headers.remove_all("content-encoding");
        if input.body_file_mode == ResponseBodyFileMode::Linked {
            return self.create_linked(
                input.id,
                input.revision,
                input.status,
                &headers,
                input.media_type,
                ResponseAssetProvenance::Imported,
                LinkedResponseBody {
                    path: info.path,
                    original_content_codings: Vec::new(),
                    apply_original_compression: false,
                },
                info.bytes,
            );
        }
        self.create_from_reader(
            input.id,
            input.revision,
            input.status,
            &headers,
            input.media_type,
            ResponseAssetProvenance::Imported,
            Some(info.path),
            ExactFileReader::new(file, info.bytes),
        )
    }

    /// Reads selected-file availability without loading its body into memory.
    ///
    /// # Errors
    /// Rejects missing, unreadable, non-regular or oversized files.
    pub fn body_file_info(
        &self,
        path: &Path,
        reference: Option<&str>,
    ) -> Result<ResponseBodyFileInfo, AppError> {
        let (_, mut info) = open_selected_file(path)?;
        info.error = reference.and_then(|key| {
            self.file_errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(key)
                .cloned()
        });
        Ok(info)
    }

    /// Copies one exact complete retained body under an eviction lease.
    ///
    /// # Errors
    /// Incomplete, truncated, lossy, evicted, or disabled boundaries fail.
    pub async fn create_from_session(
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
        if let Some(path) = input.body_path {
            if encoded_body_override.is_some() {
                return Err(invalid("Choose a replacement file or edited text."));
            }
            let (file, info) = open_selected_file(&path)?;
            let mut headers = headers.clone();
            headers.remove_all("content-encoding");
            let asset = ResponseAsset {
                id: input.id,
                revision: input.revision,
                status,
                headers,
                body_bytes: info.bytes,
                sha256: String::new(),
                media_type,
                provenance,
                linked_body: None,
                copied_from: None,
            };
            return self
                .create_file_revision(
                    asset,
                    file,
                    info,
                    input.body_file_mode,
                    metadata.content_codings,
                    input.preserve_content_encoding,
                )
                .await;
        }
        match encoded_body_override {
            Some(body) => self.create_from_reader(
                input.id,
                input.revision,
                status,
                headers,
                media_type,
                provenance,
                None,
                std::io::Cursor::new(body),
            ),
            None if metadata.retained_bytes == 0 => self.create_from_reader(
                input.id,
                input.revision,
                status,
                headers,
                media_type,
                provenance,
                None,
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
                    None,
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
        copied_from: Option<PathBuf>,
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
            linked_body: None,
            copied_from,
        };
        self.persist_asset(asset)
    }

    #[allow(clippy::too_many_arguments)]
    fn create_linked(
        &self,
        id: String,
        revision: u64,
        status: u16,
        headers: &HeaderBlock,
        media_type: Option<String>,
        provenance: ResponseAssetProvenance,
        linked: LinkedResponseBody,
        bytes: u64,
    ) -> Result<ResponseAsset, AppError> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_identity(&id, revision)?;
        if !(200..=599).contains(&status)
            || headers.iter().count() > 256
            || headers
                .iter()
                .map(|f| f.name().len() + f.value().len())
                .sum::<usize>()
                > 64 * 1024
        {
            return Err(invalid(
                "Response status or headers exceed the editor limits.",
            ));
        }
        if !linked.path.is_absolute() {
            return Err(invalid("Linked response file needs an absolute path."));
        }
        if self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&format!("{id}@{revision}"))
        {
            return Err(invalid("Saved response revision already exists."));
        }
        let mut headers = repair_headers(headers, bytes, media_type.as_deref())?;
        let applied = if linked.apply_original_compression {
            linked.original_content_codings.as_slice()
        } else {
            &[]
        };
        compression_stack(applied)
            .map_err(|message| AppError::new(ErrorCategory::InvalidInput, message, false))?;
        set_content_encoding(&mut headers, applied)?;
        self.persist_asset(ResponseAsset {
            id,
            revision,
            status,
            headers,
            body_bytes: bytes,
            sha256: String::new(),
            media_type,
            provenance,
            linked_body: Some(linked),
            copied_from: None,
        })
    }

    fn persist_asset(&self, asset: ResponseAsset) -> Result<ResponseAsset, AppError> {
        let root = self
            .root
            .as_deref()
            .ok_or_else(|| unavailable("Saved response storage is unavailable."))?;
        let mut next = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        next.insert(asset.asset_ref(), asset.clone());
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

pub(crate) fn asset_codings(headers: &HeaderBlock) -> Vec<String> {
    headers
        .values("content-encoding")
        .filter_map(|value| std::str::from_utf8(value).ok())
        .flat_map(|value| value.split(','))
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value != "identity" && !value.is_empty())
        .collect()
}

/// Sets the representation header from the exact stack used to encode edited bytes.
pub(crate) fn set_content_encoding(
    headers: &mut HeaderBlock,
    codings: &[String],
) -> Result<(), AppError> {
    headers.remove_all("content-encoding");
    if !codings.is_empty() {
        headers.push(
            HeaderField::try_new("content-encoding", codings.join(", "))
                .map_err(|_| invalid("Response content encoding is invalid."))?,
        );
    }
    Ok(())
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
        if asset.linked_body.is_some() {
            return Ok(());
        }
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
        if let Some(linked) = &asset.linked_body {
            let result = self.resolve_linked(&asset, linked);
            if let Err(message) = &result {
                self.file_errors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(asset.asset_ref(), message.clone());
            }
            return result;
        }
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
            headers: repair_headers(
                &asset.headers,
                asset.body_bytes,
                asset.media_type.as_deref(),
            )
            .map_err(|error| error.to_string())?,
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

impl ResponseAssetStore {
    /// Opens the exact configured byte source for read-only comparison acquisition.
    /// Missing files retain their saved metadata; callers must distinguish encoded
    /// copied bytes from decoded linked-file input and validate a coherent read.
    pub(crate) fn comparison_source(
        &self,
        reference: &str,
    ) -> Result<(ResponseAsset, Option<File>), AppError> {
        let asset = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(reference)
            .cloned()
            .ok_or_else(|| unavailable("Saved response revision is unavailable"))?;
        let path = asset
            .linked_body
            .as_ref()
            .map(|link| link.path.clone())
            .or_else(|| {
                self.root
                    .as_deref()
                    .map(|root| body_path(root, &asset.sha256))
            });
        let file = path.and_then(|path| File::open(path).ok());
        Ok((asset, file))
    }
    /// Materializes bounded, stable bytes without exposing an external path to a browser.
    pub(crate) async fn freeze_preview(
        &self,
        reference: &str,
        destination: &Path,
        limit: u64,
        canceled: &std::sync::atomic::AtomicBool,
    ) -> Result<(ResponseAsset, u64, String), AppError> {
        let asset = self
            .assets
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(reference)
            .cloned()
            .ok_or_else(|| unavailable("Saved response revision is unavailable"))?;
        let path = match &asset.linked_body {
            Some(link) => link.path.clone(),
            None => body_path(
                self.root
                    .as_deref()
                    .ok_or_else(|| unavailable("Response storage is unavailable"))?,
                &asset.sha256,
            ),
        };
        let mut file = File::open(path).map_err(|_| {
            unavailable(
                "Response file is unavailable; select a readable file and save the rule again",
            )
        })?;
        let before = file
            .metadata()
            .map_err(|_| unavailable("Response file metadata is unavailable"))?;
        if !before.is_file() || before.len() > limit || limit == 0 {
            return Err(unavailable(
                "Response exceeds the preview limit (32 MiB per resource, 256 MiB total)",
            ));
        }
        let read = |file: &mut File| -> Result<Vec<u8>, AppError> {
            let mut bytes = Vec::new();
            let mut buffer = vec![0; 16 * 1024];
            loop {
                if canceled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err(unavailable("Captured page preview canceled"));
                }
                let count = file
                    .read(&mut buffer)
                    .map_err(|_| unavailable("Response file could not be read"))?;
                if count == 0 {
                    break;
                }
                if bytes.len() as u64 + count as u64 > limit {
                    return Err(unavailable("Response grew beyond the preview byte limit"));
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Ok(bytes)
        };
        let bytes = read(&mut file)?;
        file.rewind()
            .map_err(|_| unavailable("Response file could not be verified"))?;
        let verified = read(&mut file)?;
        let after = file
            .metadata()
            .map_err(|_| unavailable("Response file metadata is unavailable"))?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if bytes != verified
            || before.len() != after.len()
            || bytes.len() as u64 != before.len()
            || before.modified().ok() != after.modified().ok()
            || (asset.linked_body.is_none()
                && (digest != asset.sha256 || bytes.len() as u64 != asset.body_bytes))
        {
            return Err(unavailable(
                "Response file changed during preparation; retry after saving the file",
            ));
        }
        let codings = if asset.linked_body.is_some() {
            Vec::new()
        } else {
            asset_codings(&asset.headers)
        };
        let result = crate::response_file::ResponseFile::preview(bytes, codings)
            .write_to_cancellable(destination, limit, canceled)
            .await?;
        let sha256 = format!(
            "{:x}",
            Sha256::digest(
                std::fs::read(destination)
                    .map_err(|_| unavailable("Frozen response is unavailable"))?
            )
        );
        Ok((asset, result.bytes, sha256))
    }

    fn resolve_linked(
        &self,
        asset: &ResponseAsset,
        linked: &LinkedResponseBody,
    ) -> Result<AutomationResponse, String> {
        let (file, info) = open_selected_file(&linked.path).map_err(|error| error.message)?;
        let expected = info.bytes;
        let mut headers = repair_headers(&asset.headers, expected, asset.media_type.as_deref())
            .map_err(|error| error.message)?;
        let codings = if linked.apply_original_compression {
            linked.original_content_codings.clone()
        } else {
            Vec::new()
        };
        let encoders = content_encoders(&codings)?;
        let compressed = !encoders.is_empty();
        set_content_encoding(&mut headers, &codings).map_err(|error| error.message)?;
        if compressed {
            headers.remove_all("content-length");
        }
        let head = ResponseHead {
            status: asset.status,
            headers,
            source_version: HttpLegVersion::Http1,
        };
        let (sender, body) =
            BodyStream::channel(NonZeroUsize::new(8).expect("file stream capacity is nonzero"));
        let errors = self.file_errors.clone();
        let reference = asset.asset_ref();
        std::thread::Builder::new()
            .name("transmog-linked-response".into())
            .spawn(move || {
                let result = if compressed {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| {
                            BodyStreamError::Failed(
                                "Response compression worker could not start.".into(),
                            )
                        })
                        .and_then(|runtime| {
                            runtime.block_on(encode_file_frames(
                                file,
                                expected,
                                encoders,
                                EncodedSink::Stream(&sender),
                            ))
                        })
                } else {
                    stream_file_handle(file, expected, &sender)
                };
                if let Err(error) = result {
                    let message = error.to_string();
                    if !message.contains("consumer closed") {
                        errors
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(reference, message);
                    }
                    let _ = sender.blocking_send(Err(error));
                } else {
                    errors
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&reference);
                }
            })
            .map_err(|_| "Response file stream could not start. Try again.".to_owned())?;
        Ok(AutomationResponse::Streaming(LocalStreamingResponse {
            head,
            body,
            body_length: if compressed { None } else { Some(expected) },
        }))
    }
}

fn compression_stack(codings: &[String]) -> Result<ContentCodingStack, String> {
    let headers = if codings.is_empty() {
        HeaderBlock::new()
    } else {
        HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-encoding", codings.join(", "))
                .map_err(|_| "Original compression metadata is invalid.".to_owned())?,
        ])
    };
    ContentCodingStack::from_headers(&headers, ContentLimits::default().max_coding_layers())
        .map_err(|error| error.to_string())
}

fn content_encoders(codings: &[String]) -> Result<Vec<ContentEncoder>, String> {
    let defaults = ContentLimits::default();
    let bytes = NonZeroUsize::new(usize::try_from(MAX_ASSET_BYTES).expect("file limit fits usize"))
        .expect("asset limit is nonzero");
    let limits = ContentLimits::new(
        bytes,
        bytes,
        bytes,
        defaults.max_decoder_window_bytes(),
        defaults.max_expansion_ratio(),
        defaults.expansion_slack_bytes(),
        defaults.max_coding_layers(),
    );
    compression_stack(codings)?
        .encode_order()
        .map(|coding| ContentEncoder::new(coding, limits).map_err(|error| error.to_string()))
        .collect()
}

enum EncodedSink<'a> {
    File(&'a mut File),
    Stream(&'a transmog_core::BodyStreamSender),
}
impl EncodedSink<'_> {
    async fn write(&mut self, frames: Vec<BodyFrame>) -> Result<(), BodyStreamError> {
        for frame in frames {
            match self {
                Self::File(file) => {
                    if let BodyFrame::Data(bytes) = frame {
                        file.write_all(&bytes).map_err(|_| {
                            BodyStreamError::Failed(
                                "Compressed response file could not be written.".into(),
                            )
                        })?;
                    }
                }
                Self::Stream(sender) => sender
                    .send(Ok(frame))
                    .await
                    .map_err(|_| BodyStreamError::Failed("response consumer closed".into()))?,
            }
        }
        Ok(())
    }
}

async fn encode_chain(
    encoders: &mut [ContentEncoder],
    mut frames: Vec<BodyFrame>,
) -> Result<Vec<BodyFrame>, BodyStreamError> {
    for encoder in encoders {
        let mut next = Vec::new();
        for frame in frames {
            next.extend(
                encoder
                    .on_frame(frame)
                    .await
                    .map_err(|error| BodyStreamError::Failed(error.to_string()))?,
            );
        }
        frames = next;
    }
    Ok(frames)
}

async fn encode_file_frames(
    file: File,
    expected: u64,
    mut encoders: Vec<ContentEncoder>,
    mut sink: EncodedSink<'_>,
) -> Result<(), BodyStreamError> {
    let mut reader = ExactFileReader::new(file, expected);
    let mut buffer = vec![0; STREAM_CHUNK_BYTES];
    loop {
        let read=reader.read(&mut buffer).map_err(|_|BodyStreamError::Failed("Response file changed or became unreadable while serving. Try again or choose another file.".into()))?;
        if read == 0 {
            break;
        }
        if let EncodedSink::Stream(sender) = &sink
            && sender.is_closed()
        {
            return Err(BodyStreamError::Failed("response consumer closed".into()));
        }
        let frames = encode_chain(
            &mut encoders,
            vec![BodyFrame::Data(Bytes::copy_from_slice(&buffer[..read]))],
        )
        .await?;
        sink.write(frames).await?;
    }
    for index in 0..encoders.len() {
        let frames = encoders[index]
            .finish()
            .await
            .map_err(|error| BodyStreamError::Failed(error.to_string()))?;
        let frames = encode_chain(&mut encoders[index + 1..], frames).await?;
        sink.write(frames).await?;
    }
    Ok(())
}

fn encode_file_temporary(
    file: File,
    expected: u64,
    codings: &[String],
    root: &Path,
) -> Result<tempfile::NamedTempFile, AppError> {
    let mut output = tempfile::NamedTempFile::new_in(root)
        .map_err(|_| unavailable("Response compression temporary file is unavailable."))?;
    let encoders = content_encoders(codings)
        .map_err(|message| AppError::new(ErrorCategory::InvalidInput, message, false))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| unavailable("Response compression worker could not start."))?;
    runtime
        .block_on(encode_file_frames(
            file,
            expected,
            encoders,
            EncodedSink::File(output.as_file_mut()),
        ))
        .map_err(|error| AppError::new(ErrorCategory::Unavailable, error.to_string(), false))?;
    output
        .as_file_mut()
        .rewind()
        .map_err(|_| unavailable("Compressed response could not be read."))?;
    Ok(output)
}

fn open_selected_file(path: &Path) -> Result<(File, ResponseBodyFileInfo), AppError> {
    let file = File::open(path).map_err(|_| {
        unavailable(
            "Response file is missing or unreadable. Restore access or choose another file.",
        )
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| unavailable("Response file metadata could not be read."))?;
    if !metadata.is_file() || metadata.len() > MAX_ASSET_BYTES {
        return Err(invalid(
            "Choose a regular response file no larger than one GiB.",
        ));
    }
    let path = std::path::absolute(path).map_err(|_| invalid("Response file path is invalid."))?;
    Ok((
        file,
        ResponseBodyFileInfo {
            path,
            bytes: metadata.len(),
            error: None,
        },
    ))
}

struct ExactFileReader {
    file: File,
    expected: u64,
    read: u64,
}
impl ExactFileReader {
    fn new(file: File, expected: u64) -> Self {
        Self {
            file,
            expected,
            read: 0,
        }
    }
}
impl Read for ExactFileReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let read = self.file.read(buffer)?;
        self.read += read as u64;
        if self.read > self.expected || (read == 0 && self.read != self.expected) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Response file changed while being copied.",
            ));
        }
        Ok(read)
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
    let result = File::open(path)
        .map_err(|_| BodyStreamError::Failed("response asset body open failed".to_owned()))
        .and_then(|file| stream_file_handle(file, expected, sender));
    if let Err(error) = result {
        let _ = sender.blocking_send(Err(error));
    }
}

fn stream_file_handle(
    mut file: File,
    expected: u64,
    sender: &transmog_core::BodyStreamSender,
) -> Result<(), BodyStreamError> {
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| BodyStreamError::Failed("response asset body read failed".to_owned()))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| BodyStreamError::Failed("response asset length overflow".to_owned()))?;
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
        // This is a new local response. Its encoding describes the stored bytes;
        // an old Connection nomination cannot remove that representation metadata.
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
        ) || (name != "content-encoding" && nominated.iter().any(|nominated| nominated == &name))
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
        if asset
            .linked_body
            .as_ref()
            .is_some_and(|linked| linked.path.is_absolute())
            || std::fs::metadata(body_path(root, &asset.sha256))
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
mod preview_snapshot_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[tokio::test]
    async fn linked_preview_copies_bytes_and_reports_missing_changed_limits_and_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().join("assets"))).unwrap();
        let file = root.path().join("linked.css");
        let payload = vec![b'a'; 25_000_001];
        std::fs::write(&file, &payload).unwrap();
        let asset = store
            .import_file(ImportResponseAsset {
                id: "linked".into(),
                revision: 1,
                body_path: file.clone(),
                body_file_mode: ResponseBodyFileMode::Linked,
                status: 200,
                headers: HeaderBlock::new(),
                media_type: Some("text/css".into()),
            })
            .unwrap();
        assert!(asset.linked_body.is_some());
        let output = root.path().join("frozen");
        let (_, bytes, hash) = store
            .freeze_preview(
                &asset.asset_ref(),
                &output,
                32 * 1024 * 1024,
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        assert_eq!(bytes, payload.len() as u64);
        assert_eq!(hash, format!("{:x}", Sha256::digest(&payload)));
        std::fs::write(&file, b"edited").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), payload);
        assert!(
            store
                .freeze_preview(
                    &asset.asset_ref(),
                    &root.path().join("limited"),
                    2,
                    &AtomicBool::new(false)
                )
                .await
                .is_err()
        );
        assert!(
            store
                .freeze_preview(
                    &asset.asset_ref(),
                    &root.path().join("canceled"),
                    32 * 1024 * 1024,
                    &AtomicBool::new(true)
                )
                .await
                .err()
                .unwrap()
                .message
                .contains("canceled")
        );
        std::fs::remove_file(&file).unwrap();
        assert!(
            store
                .freeze_preview(
                    &asset.asset_ref(),
                    &root.path().join("missing"),
                    32 * 1024 * 1024,
                    &AtomicBool::new(false)
                )
                .await
                .err()
                .unwrap()
                .message
                .contains("unavailable")
        );
        assert_eq!(std::fs::read(&output).unwrap(), payload);
    }
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

    #[tokio::test]
    async fn compressed_file_replacement_is_independent_of_the_text_edit_limit() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().into())).unwrap();
        let source = store
            .create_authored(AuthoredResponseAsset {
                id: "large-compression".into(),
                revision: 1,
                status: 200,
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("content-encoding", "gzip").unwrap(),
                ]),
                body: crate::inspector::encode_content(&["gzip".into()], b"source".to_vec())
                    .await
                    .unwrap(),
                media_type: Some("application/octet-stream".into()),
            })
            .unwrap();
        let path = root.path().join("large.input");
        let payload = vec![0x5a; MAX_AUTHORED_BYTES + 1];
        std::fs::write(&path, &payload).unwrap();
        let asset = store
            .edit(ResponseAssetEdit {
                asset_reference: source.asset_ref(),
                status: 200,
                headers: HeaderBlock::new(),
                media_type: source.media_type,
                decoded_body: None,
                body_path: Some(path),
                body_file_mode: crate::ResponseBodyFileMode::Copy,
                preserve_content_encoding: true,
            })
            .await
            .unwrap();
        assert!(asset.linked_body.is_none());
        let (_, body) = returned_body(&store, &asset).await;
        let defaults = ContentLimits::default();
        let bytes = NonZeroUsize::new(32 * 1024 * 1024).unwrap();
        let limits = ContentLimits::new(
            bytes,
            bytes,
            bytes,
            defaults.max_decoder_window_bytes(),
            NonZeroUsize::new(10000).unwrap(),
            defaults.expansion_slack_bytes(),
            defaults.max_coding_layers(),
        );
        let mut decoder =
            transmog_content::ContentDecoder::new(transmog_content::ContentCoding::Gzip, limits)
                .unwrap();
        let mut frames = decoder
            .on_frame(BodyFrame::Data(Bytes::from(body)))
            .await
            .unwrap();
        frames.extend(decoder.finish().await.unwrap());
        let restored = frames
            .into_iter()
            .filter_map(|frame| {
                if let BodyFrame::Data(bytes) = frame {
                    Some(bytes)
                } else {
                    None
                }
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(restored, payload);
    }

    #[tokio::test]
    async fn binary_replacement_files_apply_every_original_compression_stack() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().into())).unwrap();
        let payload = (0..32768)
            .map(|index| u8::try_from(index % 256).unwrap())
            .collect::<Vec<_>>();
        for (index, codings) in [
            vec!["gzip"],
            vec!["x-gzip"],
            vec!["br"],
            vec!["zstd"],
            vec!["deflate"],
            vec!["gzip", "br"],
            vec!["x-gzip", "br"],
        ]
        .into_iter()
        .enumerate()
        {
            let codings = codings.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let source = store
                .create_authored(AuthoredResponseAsset {
                    id: format!("compression-{index}"),
                    revision: 1,
                    status: 200,
                    headers: HeaderBlock::from_fields(vec![
                        HeaderField::try_new("content-encoding", codings.join(", ")).unwrap(),
                    ]),
                    body: crate::inspector::encode_content(&codings, b"source".to_vec())
                        .await
                        .unwrap(),
                    media_type: Some("application/octet-stream".into()),
                })
                .unwrap();
            for linked in [false, true] {
                let path = root.path().join(format!("replacement-{index}-{linked}"));
                if linked {
                    File::create(&path).unwrap().set_len(25_000_001).unwrap();
                } else {
                    std::fs::write(&path, &payload).unwrap();
                }
                let asset = store
                    .edit(ResponseAssetEdit {
                        asset_reference: source.asset_ref(),
                        status: 200,
                        headers: stale_entity_headers(),
                        media_type: source.media_type.clone(),
                        decoded_body: None,
                        body_path: Some(path.clone()),
                        body_file_mode: if linked {
                            ResponseBodyFileMode::Linked
                        } else {
                            ResponseBodyFileMode::Copy
                        },
                        preserve_content_encoding: true,
                    })
                    .await
                    .unwrap();
                assert_eq!(asset.linked_body.is_some(), linked);
                if linked {
                    std::fs::write(&path, &payload).unwrap();
                }
                let (headers, encoded) = returned_body(&store, &asset).await;
                assert_eq!(asset_codings(&headers), codings);
                assert_eq!(
                    crate::inspector::decode_content(&codings, encoded)
                        .await
                        .unwrap(),
                    payload
                );
            }
        }
    }

    #[tokio::test]
    async fn explicit_file_modes_survive_size_changes_source_removal_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().join("assets"))).unwrap();
        for size in [32, 25_000_001] {
            for mode in [ResponseBodyFileMode::Copy, ResponseBodyFileMode::Linked] {
                let id = format!("file-{size}-{mode:?}");
                let path = root.path().join(format!("{id}.input"));
                File::create(&path).unwrap().set_len(size).unwrap();
                let asset = store
                    .import_file(ImportResponseAsset {
                        id,
                        revision: 1,
                        status: 200,
                        headers: HeaderBlock::new(),
                        body_path: path.clone(),
                        body_file_mode: mode,
                        media_type: Some("application/octet-stream".into()),
                    })
                    .unwrap();
                assert_eq!(
                    asset.linked_body.is_some(),
                    mode == ResponseBodyFileMode::Linked
                );
                assert_eq!(
                    asset.copied_from.as_deref(),
                    (mode == ResponseBodyFileMode::Copy).then_some(path.as_path())
                );
                std::fs::write(&path, b"external edit").unwrap();
                let restarted = ResponseAssetStore::load(Some(root.path().join("assets"))).unwrap();
                let (headers, body) = returned_body(&restarted, &asset).await;
                if mode == ResponseBodyFileMode::Linked {
                    assert_eq!(body, b"external edit");
                    assert_eq!(headers.values("content-length").next(), Some(&b"13"[..]));
                    std::fs::remove_file(&path).unwrap();
                    let missing =
                        ResponseAssetStore::load(Some(root.path().join("assets"))).unwrap();
                    assert!(ResponseAssetResolver::validate(&missing, &asset.asset_ref()).is_ok());
                    assert!(ResponseAssetResolver::resolve(&missing, &asset.asset_ref()).is_err());
                    assert!(
                        missing
                            .inspect(&asset.asset_ref())
                            .await
                            .unwrap()
                            .asset
                            .linked_body
                            .is_some()
                    );
                    std::fs::write(&path, b"restored").unwrap();
                    assert_eq!(returned_body(&missing, &asset).await.1, b"restored");
                    assert!(
                        missing
                            .body_file_info(&path, Some(&asset.asset_ref()))
                            .unwrap()
                            .error
                            .is_none()
                    );
                } else {
                    assert_eq!(body.len() as u64, size);
                    assert!(body.iter().all(|byte| *byte == 0));
                    std::fs::remove_file(&path).unwrap();
                    let removed =
                        ResponseAssetStore::load(Some(root.path().join("assets"))).unwrap();
                    assert_eq!(
                        removed.get(&asset.asset_ref()).unwrap().copied_from,
                        Some(path)
                    );
                    assert_eq!(returned_body(&removed, &asset).await.1, body);
                }
            }
        }
    }

    #[test]
    fn replacement_file_readers_reject_growth_and_truncation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("body");
        std::fs::write(&path, b"original").unwrap();
        let file = File::open(&path).unwrap();
        let mut reader = ExactFileReader::new(file, 8);
        std::fs::write(&path, b"longer content").unwrap();
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
        std::fs::write(&path, b"short").unwrap();
        let mut reader = ExactFileReader::new(File::open(&path).unwrap(), 8);
        assert!(reader.read_to_end(&mut Vec::new()).is_err());
    }

    #[tokio::test]
    async fn saved_text_preview_honors_quoted_and_case_insensitive_charset_parameters() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().into())).unwrap();
        let text = "Hi 🌍";
        for (index, media_type) in [
            "text/plain; charset=utf-16le",
            "text/plain; Charset = \"UTF-16LE\"",
        ]
        .into_iter()
        .enumerate()
        {
            let asset = store
                .create_authored(AuthoredResponseAsset {
                    id: format!("charset-{index}"),
                    revision: 1,
                    status: 200,
                    headers: HeaderBlock::new(),
                    body: text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
                    media_type: Some(media_type.into()),
                })
                .unwrap();
            let inspection = store.inspect(&asset.asset_ref()).await.unwrap();
            assert_eq!(inspection.display, text);
            assert_eq!(inspection.text_encoding, Some("utf-16le"));
        }
    }

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("transmog-assets-{}", random_hex().unwrap()))
    }

    #[tokio::test]
    async fn binary_inspection_and_edits_preserve_complete_decoded_and_encoded_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().into())).unwrap();
        let binary = (0..=u8::MAX).collect::<Vec<_>>();
        for (index, codings) in [
            vec![],
            vec!["gzip".into()],
            vec!["gzip".into(), "br".into()],
        ]
        .into_iter()
        .enumerate()
        {
            let mut headers = HeaderBlock::new();
            set_content_encoding(&mut headers, &codings).unwrap();
            let encoded = crate::inspector::encode_content(&codings, binary.clone())
                .await
                .unwrap();
            let original = store
                .create_authored(AuthoredResponseAsset {
                    id: format!("binary-{index}"),
                    revision: 1,
                    status: 200,
                    headers,
                    body: encoded.clone(),
                    media_type: Some("application/octet-stream".into()),
                })
                .unwrap();
            let inspection = store.inspect(&original.asset_ref()).await.unwrap();
            assert_eq!(inspection.text_encoding, None);
            assert_eq!(
                STANDARD.decode(inspection.bytes_base64.unwrap()).unwrap(),
                binary
            );
            assert!(inspection.explanation.contains("Hex"));
            let retained = store
                .edit(ResponseAssetEdit {
                    asset_reference: original.asset_ref(),
                    status: 201,
                    headers: original.headers.clone(),
                    media_type: original.media_type.clone(),
                    decoded_body: None,
                    body_path: None,
                    body_file_mode: crate::ResponseBodyFileMode::Copy,
                    preserve_content_encoding: true,
                })
                .await
                .unwrap();
            assert_eq!(returned_body(&store, &retained).await.1, encoded);
            let mut changed = binary.clone();
            changed[128] = 255;
            for preserve in [true, false] {
                let edited = store
                    .edit(ResponseAssetEdit {
                        asset_reference: retained.asset_ref(),
                        status: 200,
                        headers: retained.headers.clone(),
                        media_type: retained.media_type.clone(),
                        decoded_body: Some(changed.clone()),
                        body_path: None,
                        body_file_mode: crate::ResponseBodyFileMode::Copy,
                        preserve_content_encoding: preserve,
                    })
                    .await
                    .unwrap();
                let (headers, body) = returned_body(&store, &edited).await;
                let applied = asset_codings(&headers);
                assert_eq!(applied, if preserve { codings.clone() } else { vec![] });
                assert_eq!(
                    crate::inspector::decode_content(&applied, body)
                        .await
                        .unwrap(),
                    changed
                );
            }
        }
    }

    #[tokio::test]
    async fn oversized_inspection_never_exposes_a_partial_editable_body() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().join("assets"))).unwrap();
        let path = root.path().join("large-body");
        File::create(&path)
            .unwrap()
            .set_len(MAX_AUTHORED_BYTES as u64 + 1)
            .unwrap();
        let asset = store
            .import_file(ImportResponseAsset {
                id: "large".into(),
                revision: 1,
                status: 200,
                headers: HeaderBlock::new(),
                body_path: path,
                body_file_mode: crate::ResponseBodyFileMode::Copy,
                media_type: None,
            })
            .unwrap();
        let inspection = store.inspect(&asset.asset_ref()).await.unwrap();
        assert!(inspection.bytes_base64.is_none());
        assert!(inspection.text_encoding.is_none());
        assert!(inspection.display.starts_with("00000000"));
        assert!(inspection.explanation.contains("first 4096 of"));
        assert!(inspection.explanation.contains("16 MiB"));
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

    async fn returned_body(
        store: &ResponseAssetStore,
        asset: &ResponseAsset,
    ) -> (HeaderBlock, Vec<u8>) {
        let (head, frames) =
            match ResponseAssetResolver::resolve(store, &asset.asset_ref()).unwrap() {
                AutomationResponse::Buffered(response) => (response.head, response.body),
                AutomationResponse::Streaming(mut response) => {
                    let mut frames = Vec::new();
                    while let Some(frame) = response.body.recv().await {
                        frames.push(frame.unwrap());
                    }
                    (response.head, frames)
                }
            };
        let mut body = Vec::new();
        for frame in frames {
            if let BodyFrame::Data(bytes) = frame {
                body.extend_from_slice(&bytes);
            }
        }
        let expected = body.len().to_string();
        if asset.linked_body.as_ref().is_some_and(|linked| {
            linked.apply_original_compression && !linked.original_content_codings.is_empty()
        }) {
            assert!(head.headers.values("content-length").next().is_none());
        } else {
            assert_eq!(
                head.headers.values("content-length").collect::<Vec<_>>(),
                vec![expected.as_bytes()]
            );
        }
        if asset.linked_body.is_none() {
            assert_eq!(asset.body_bytes, u64::try_from(body.len()).unwrap());
        }
        (head.headers, body)
    }

    fn stale_entity_headers() -> HeaderBlock {
        HeaderBlock::from_fields(vec![
            HeaderField::try_new("Connection", "Content-Encoding").unwrap(),
            HeaderField::try_new("Content-Length", "999").unwrap(),
            HeaderField::try_new("content-length", "1").unwrap(),
            HeaderField::try_new("Content-Encoding", "incorrect").unwrap(),
            HeaderField::try_new("content-encoding", "br").unwrap(),
        ])
    }

    #[tokio::test]
    async fn edited_assets_return_framing_for_the_actual_compressed_or_identity_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().into())).unwrap();
        for (index, codings) in [
            vec![],
            vec!["gzip"],
            vec!["br"],
            vec!["deflate"],
            vec!["zstd"],
            vec!["gzip", "br"],
        ]
        .into_iter()
        .enumerate()
        {
            let codings = codings.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let mut headers = HeaderBlock::new();
            set_content_encoding(&mut headers, &codings).unwrap();
            let original = store
                .create_authored(AuthoredResponseAsset {
                    id: format!("source-{index}"),
                    revision: 1,
                    status: 200,
                    headers,
                    body: crate::inspector::encode_content(&codings, b"original".to_vec())
                        .await
                        .unwrap(),
                    media_type: None,
                })
                .unwrap();
            let decoded = "changed UTF-8 response: café".as_bytes().to_vec();
            let edit = ResponseAssetEdit {
                asset_reference: original.asset_ref(),
                status: 200,
                headers: stale_entity_headers(),
                media_type: None,
                decoded_body: Some(decoded.clone()),
                body_path: None,
                body_file_mode: crate::ResponseBodyFileMode::Copy,
                preserve_content_encoding: true,
            };
            let compressed = store.edit(edit).await.unwrap();
            let (headers, body) = returned_body(&store, &compressed).await;
            assert_eq!(asset_codings(&headers), codings);
            assert_eq!(
                headers.values("content-encoding").count(),
                usize::from(!codings.is_empty())
            );
            let actual = if codings.is_empty() {
                body
            } else {
                crate::inspector::decode_content(&codings, body)
                    .await
                    .unwrap()
            };
            assert_eq!(actual, decoded);

            let identity = store
                .edit(ResponseAssetEdit {
                    asset_reference: compressed.asset_ref(),
                    status: 201,
                    headers: stale_entity_headers(),
                    media_type: None,
                    decoded_body: Some(decoded.clone()),
                    body_path: None,
                    body_file_mode: crate::ResponseBodyFileMode::Copy,
                    preserve_content_encoding: false,
                })
                .await
                .unwrap();
            let (headers, body) = returned_body(&store, &identity).await;
            assert!(headers.values("content-encoding").next().is_none());
            assert_eq!(body, decoded);

            let retained = store
                .edit(ResponseAssetEdit {
                    asset_reference: original.asset_ref(),
                    status: 202,
                    headers: stale_entity_headers(),
                    media_type: None,
                    decoded_body: None,
                    body_path: None,
                    body_file_mode: crate::ResponseBodyFileMode::Copy,
                    preserve_content_encoding: false,
                })
                .await
                .unwrap();
            let (headers, body) = returned_body(&store, &retained).await;
            assert_eq!(asset_codings(&headers), codings);
            assert_eq!(
                body,
                std::fs::read(body_path(root.path(), &original.sha256)).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn replacement_files_and_empty_edits_clear_encoding_and_repair_returned_lengths() {
        let root = tempfile::tempdir().unwrap();
        let store = ResponseAssetStore::load(Some(root.path().into())).unwrap();
        let source = store
            .create_authored(AuthoredResponseAsset {
                id: "replacement".into(),
                revision: 1,
                status: 200,
                headers: HeaderBlock::from_fields(vec![
                    HeaderField::try_new("content-encoding", "gzip").unwrap(),
                ]),
                body: crate::inspector::encode_content(&["gzip".into()], b"original".to_vec())
                    .await
                    .unwrap(),
                media_type: None,
            })
            .unwrap();
        let file = root.path().join("body.input");
        let expected = vec![0x5a; usize::try_from(BUFFERED_RESPONSE_BYTES).unwrap() + 17];
        std::fs::write(&file, &expected).unwrap();
        let replacement = store
            .edit(ResponseAssetEdit {
                asset_reference: source.asset_ref(),
                status: 200,
                headers: stale_entity_headers(),
                media_type: None,
                decoded_body: None,
                body_path: Some(file),
                body_file_mode: crate::ResponseBodyFileMode::Copy,
                preserve_content_encoding: false,
            })
            .await
            .unwrap();
        let (headers, body) = returned_body(&store, &replacement).await;
        assert!(headers.values("content-encoding").next().is_none());
        assert_eq!(body, expected);
        let empty = store
            .edit(ResponseAssetEdit {
                asset_reference: source.asset_ref(),
                status: 200,
                headers: stale_entity_headers(),
                media_type: None,
                decoded_body: Some(vec![]),
                body_path: None,
                body_file_mode: crate::ResponseBodyFileMode::Copy,
                preserve_content_encoding: false,
            })
            .await
            .unwrap();
        // Even an older stored header block must not override framing on return.
        store
            .assets
            .write()
            .unwrap()
            .get_mut(&empty.asset_ref())
            .unwrap()
            .headers = HeaderBlock::from_fields(vec![
            HeaderField::try_new("Content-Length", "999").unwrap(),
            HeaderField::try_new("content-length", "1").unwrap(),
        ]);
        let (headers, body) = returned_body(&store, &empty).await;
        assert!(body.is_empty());
        assert!(headers.values("content-encoding").next().is_none());
        assert_eq!(headers.values("content-length").next(), Some(&b"0"[..]));
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
                body_file_mode: crate::ResponseBodyFileMode::Copy,
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
            storage: crate::BufferStorage::Memory,
            use_product_preferences: false,
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
                    body_path: None,
                    body_file_mode: crate::ResponseBodyFileMode::Copy,
                    preserve_content_encoding: true,
                },
                response.status,
                &response.headers,
                None,
                None,
            )
            .await
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
