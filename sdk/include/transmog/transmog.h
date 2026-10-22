#pragma once
#include "transmog/export.h"
#include "transmog/models.h"
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
#define TRANSMOG_NOEXCEPT noexcept
extern "C" {
#else
#define TRANSMOG_NOEXCEPT
#endif

typedef struct transmog_sdk_application transmog_sdk_application;
typedef struct transmog_sdk_result transmog_sdk_result;
typedef struct transmog_sdk_session_updates transmog_sdk_session_updates;
typedef struct transmog_sdk_body_page transmog_sdk_body_page;
typedef struct transmog_sdk_bytes {
  const uint8_t *data;
  size_t size;
} transmog_sdk_bytes;
// category matches the C++ ErrorCategory numeric values; message is borrowed.
typedef struct transmog_sdk_error {
  uint32_t category;
  transmog_sdk_bytes message;
  uint32_t retryable;
} transmog_sdk_error;
// Every pointer is borrowed until body_page_free. Optional text uses a null
// pointer for absence, including when its length is zero. Numeric enum values
// match C++ BodyAvailability. Boolean flags are explicitly 32-bit 0/1 values.
typedef struct transmog_sdk_body_page_view {
  transmog_sdk_bytes bytes;
  uint64_t offset;
  uint64_t total_bytes;
  size_t read_limit;
  uint32_t availability;
  uint32_t length_known;
  uint64_t observed_bytes;
  uint64_t retained_bytes;
  uint32_t has_wire_body_bytes;
  uint64_t wire_body_bytes;
  transmog_sdk_bytes exchange_id;
  transmog_sdk_bytes boundary;
  transmog_sdk_bytes media_type;
  transmog_sdk_bytes charset;
  transmog_sdk_bytes sha256;
  transmog_sdk_bytes reason;
  size_t content_coding_count;
} transmog_sdk_body_page_view;
// Typed borrowed hint on a native thread. Copy fields before returning. Free
// joins an in-flight callback except when called by that callback itself.
typedef void (*transmog_sdk_session_update_callback)(
    void *, const transmog_sdk_session_hint *);
TRANSMOG_API uint32_t transmog_sdk_abi_version(void) TRANSMOG_NOEXCEPT;
// Null means wrapper allocation failed; otherwise query valid/status.
// Creation uses AppConfig::default. It does not mutate host proxy/trust
// settings.
TRANSMOG_API transmog_sdk_application *
transmog_sdk_application_create(void) TRANSMOG_NOEXCEPT;
// Opt-in product body store at a UTF-8 caller-owned cache path. Inspect
// valid/status for construction errors. Default create retains no bodies.
TRANSMOG_API transmog_sdk_application *
transmog_sdk_application_create_with_body_store(const uint8_t *,
                                                size_t) TRANSMOG_NOEXCEPT;
TRANSMOG_API int32_t transmog_sdk_application_valid(
    const transmog_sdk_application *) TRANSMOG_NOEXCEPT;
// Best-effort shutdown; use application_shutdown first to inspect shutdown
// errors.
TRANSMOG_API void
transmog_sdk_application_free(transmog_sdk_application *) TRANSMOG_NOEXCEPT;
#define TRANSMOG_OPERATION(name)                                               \
  TRANSMOG_API transmog_sdk_result *transmog_sdk_application_##name(           \
      const transmog_sdk_application *) TRANSMOG_NOEXCEPT;
TRANSMOG_OPERATION(status)
TRANSMOG_OPERATION(product_state)
TRANSMOG_OPERATION(stop_proxy)
TRANSMOG_OPERATION(shutdown)
#undef TRANSMOG_OPERATION
// The complete generated view and its nonempty arrays/text must remain readable
// and correctly aligned throughout the call. Inputs are copied and validated by
// the canonical Rust model. The aggregate input bound is 256 KiB.
TRANSMOG_API transmog_sdk_result *transmog_sdk_application_save_product_state(
    const transmog_sdk_application *,
    const transmog_sdk_product_state *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *transmog_sdk_application_start_proxy(
    const transmog_sdk_application *,
    const transmog_sdk_proxy_start_request *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *transmog_sdk_application_query_sessions(
    const transmog_sdk_application *,
    const transmog_sdk_session_query_input *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *transmog_sdk_application_query_traffic_view(
    const transmog_sdk_application *,
    const transmog_sdk_session_query_input *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *
transmog_sdk_application_session_detail(const transmog_sdk_application *,
                                        const uint8_t *,
                                        size_t) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *
transmog_sdk_application_release_traffic_view(const transmog_sdk_application *,
                                              const uint8_t *,
                                              size_t) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *
transmog_sdk_default_proxy_start_request(void) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *
transmog_sdk_default_session_query_input(void) TRANSMOG_NOEXCEPT;
// Body reads use typed arguments/results. String inputs are length-delimited
// UTF-8, with a combined 256 KiB transfer bound. max_bytes is a per-call
// budget. Null means C wrapper allocation failed before invoking the operation.
TRANSMOG_API transmog_sdk_body_page *transmog_sdk_application_read_body_range(
    const transmog_sdk_application *, const uint8_t *session_id,
    size_t session_length, const uint8_t *boundary, size_t boundary_length,
    uint64_t offset, size_t max_bytes) TRANSMOG_NOEXCEPT;
TRANSMOG_API int32_t transmog_sdk_body_page_succeeded(
    const transmog_sdk_body_page *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_error
transmog_sdk_body_page_error(const transmog_sdk_body_page *) TRANSMOG_NOEXCEPT;
// Failed/empty owners return a zero-initialized view; inspect success/error.
TRANSMOG_API transmog_sdk_body_page_view transmog_sdk_body_page_get_view(
    const transmog_sdk_body_page *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_bytes transmog_sdk_body_page_content_coding(
    const transmog_sdk_body_page *, size_t index) TRANSMOG_NOEXCEPT;
TRANSMOG_API void
transmog_sdk_body_page_free(transmog_sdk_body_page *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *transmog_sdk_application_traffic_view_rows(
    const transmog_sdk_application *, const uint8_t *view_id, size_t id_length,
    size_t offset, size_t limit) TRANSMOG_NOEXCEPT;
// Subscription ownership is independent of the application. A null pointer
// means wrapper allocation failed; otherwise valid/status report creation
// success.
TRANSMOG_API transmog_sdk_session_updates *
transmog_sdk_application_subscribe_session_updates(
    const transmog_sdk_application *, transmog_sdk_session_update_callback,
    void *context) TRANSMOG_NOEXCEPT;
TRANSMOG_API int32_t transmog_sdk_session_updates_valid(
    const transmog_sdk_session_updates *) TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_result *transmog_sdk_session_updates_status(
    const transmog_sdk_session_updates *) TRANSMOG_NOEXCEPT;
TRANSMOG_API void transmog_sdk_session_updates_free(
    transmog_sdk_session_updates *) TRANSMOG_NOEXCEPT;
TRANSMOG_API int32_t transmog_sdk_result_succeeded(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
// Borrowed typed views. The requested type must match the operation's result;
// mismatch/failure returns null. AppError reads the original Rust failure
// model. All nested pointers expire when the result is freed or replaced.
TRANSMOG_API const transmog_sdk_app_status *transmog_sdk_result_as_app_status(
    const transmog_sdk_result *) TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_product_state *
transmog_sdk_result_as_product_state(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_proxy_start_request *
transmog_sdk_result_as_proxy_start_request(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_session_query_input *
transmog_sdk_result_as_session_query_input(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_session_page *
transmog_sdk_result_as_session_page(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_traffic_view *
transmog_sdk_result_as_traffic_view(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_session_summary *
transmog_sdk_result_as_session_summary(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_session_detail *
transmog_sdk_result_as_session_detail(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_session_hint *
transmog_sdk_result_as_session_hint(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_app_error *
transmog_sdk_result_as_app_error(const transmog_sdk_result *) TRANSMOG_NOEXCEPT;
TRANSMOG_API const transmog_sdk_slice_session_summary *
transmog_sdk_result_as_session_rows(const transmog_sdk_result *)
    TRANSMOG_NOEXCEPT;
TRANSMOG_API transmog_sdk_error
transmog_sdk_result_error(const transmog_sdk_result *) TRANSMOG_NOEXCEPT;
TRANSMOG_API void
transmog_sdk_result_free(transmog_sdk_result *) TRANSMOG_NOEXCEPT;
#ifdef __cplusplus
}
#endif
#undef TRANSMOG_NOEXCEPT
