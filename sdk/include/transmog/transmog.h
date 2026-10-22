#pragma once
#include "transmog/export.h"
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
// Borrowed SessionHint JSON on a native thread. Copy before returning, contain
// language exceptions, and keep callback/context alive until
// session_updates_free completes. Free waits for in-flight delivery, except
// from that callback itself.
typedef void (*transmog_sdk_session_update_callback)(void *, const uint8_t *,
                                                     size_t);
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
// Inputs are existing application JSON models; detail/release take UTF-8 IDs.
// Memory must remain readable throughout each call. Input limit is 256 KiB.
#define TRANSMOG_INPUT_OPERATION(name)                                         \
  TRANSMOG_API transmog_sdk_result *transmog_sdk_application_##name(           \
      const transmog_sdk_application *, const uint8_t *, size_t)               \
      TRANSMOG_NOEXCEPT;
TRANSMOG_INPUT_OPERATION(save_product_state)
TRANSMOG_INPUT_OPERATION(start_proxy)
TRANSMOG_INPUT_OPERATION(query_sessions)
TRANSMOG_INPUT_OPERATION(session_detail)
TRANSMOG_INPUT_OPERATION(read_body_range)
TRANSMOG_INPUT_OPERATION(query_traffic_view)
TRANSMOG_INPUT_OPERATION(release_traffic_view)
#undef TRANSMOG_INPUT_OPERATION
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
// Borrowed UTF-8 JSON bytes, valid until result_free; no terminating NUL
// promised.
#define TRANSMOG_BUFFER(name)                                                  \
  TRANSMOG_API const uint8_t *transmog_sdk_result_##name(                      \
      const transmog_sdk_result *) TRANSMOG_NOEXCEPT;                          \
  TRANSMOG_API size_t transmog_sdk_result_##name##_length(                     \
      const transmog_sdk_result *) TRANSMOG_NOEXCEPT;
TRANSMOG_BUFFER(json)
TRANSMOG_BUFFER(error_json)
#undef TRANSMOG_BUFFER
TRANSMOG_API void
transmog_sdk_result_free(transmog_sdk_result *) TRANSMOG_NOEXCEPT;
#ifdef __cplusplus
}
#endif
#undef TRANSMOG_NOEXCEPT
