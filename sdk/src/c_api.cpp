#include "transmog/transmog.h"
#include "transmog/transmog.hpp"
#include <new>
#include <utility>

struct transmog_sdk_result {
  transmog::Result value;
};
struct transmog_sdk_application {
  mutable transmog::Application value;
};
struct transmog_sdk_session_updates {
  transmog::SessionUpdates value;
};
struct transmog_sdk_body_page {
  transmog::BodyPage value;
};

namespace {
template <class Operation>
transmog_sdk_result *invoke(Operation operation) noexcept {
  try {
    // Reserve the wrapper before invoking an operation with side effects.
    auto *result = new (std::nothrow) transmog_sdk_result{};
    if (result)
      result->value = operation();
    return result;
  } catch (...) {
    return nullptr;
  }
}
template <class T> bool aligned(const T *value) noexcept {
  return value && reinterpret_cast<std::uintptr_t>(value) % alignof(T) == 0;
}
std::string_view input(const uint8_t *bytes, size_t length) noexcept {
  return bytes ? std::string_view(reinterpret_cast<const char *>(bytes), length)
               : std::string_view{};
}
transmog_sdk_bytes bytes(std::string_view text) noexcept {
  return {reinterpret_cast<const uint8_t *>(text.data()), text.size()};
}
transmog_sdk_bytes
optional_text(std::optional<std::string_view> text) noexcept {
  return text ? bytes(*text) : transmog_sdk_bytes{};
}
} // namespace
extern "C" {
uint32_t transmog_sdk_abi_version() noexcept { return transmog::abi_version(); }
transmog_sdk_application *transmog_sdk_application_create() noexcept {
  try {
    auto *app = new (std::nothrow) transmog_sdk_application{};
    if (app)
      app->value = transmog::Application::create();
    return app;
  } catch (...) {
    return nullptr;
  }
}
transmog_sdk_application *
transmog_sdk_application_create_with_body_store(const uint8_t *bytes,
                                                size_t length) noexcept {
  try {
    auto *app = new (std::nothrow) transmog_sdk_application{};
    if (app)
      app->value =
          transmog::Application::create_with_body_store(input(bytes, length));
    return app;
  } catch (...) {
    return nullptr;
  }
}
int32_t
transmog_sdk_application_valid(const transmog_sdk_application *app) noexcept {
  return app && app->value.valid() ? 1 : 0;
}
void transmog_sdk_application_free(transmog_sdk_application *app) noexcept {
  delete app;
}
#define TRANSMOG_OPERATION(name)                                               \
  transmog_sdk_result *transmog_sdk_application_##name(                        \
      const transmog_sdk_application *app) noexcept {                          \
    return invoke([&] {                                                        \
      return app ? transmog::Result(app->value.name()) : transmog::Result{};   \
    });                                                                        \
  }
TRANSMOG_OPERATION(status)
TRANSMOG_OPERATION(product_state)
TRANSMOG_OPERATION(stop_proxy)
TRANSMOG_OPERATION(shutdown)
#undef TRANSMOG_OPERATION
transmog_sdk_result *transmog_sdk_application_save_product_state(
    const transmog_sdk_application *app,
    const transmog_sdk_product_state *request) noexcept {
  return invoke([&] {
    return app && aligned(request)
               ? transmog::Result(app->value.save_product_state(*request))
               : transmog::Result{};
  });
}
transmog_sdk_result *transmog_sdk_application_start_proxy(
    const transmog_sdk_application *app,
    const transmog_sdk_proxy_start_request *request) noexcept {
  return invoke([&] {
    return app && aligned(request)
               ? transmog::Result(app->value.start_proxy(*request))
               : transmog::Result{};
  });
}
transmog_sdk_result *transmog_sdk_application_query_sessions(
    const transmog_sdk_application *app,
    const transmog_sdk_session_query_input *request) noexcept {
  return invoke([&] {
    return app && aligned(request)
               ? transmog::Result(app->value.query_sessions(*request))
               : transmog::Result{};
  });
}
transmog_sdk_result *transmog_sdk_application_query_traffic_view(
    const transmog_sdk_application *app,
    const transmog_sdk_session_query_input *request) noexcept {
  return invoke([&] {
    return app && aligned(request)
               ? transmog::Result(app->value.query_traffic_view(*request))
               : transmog::Result{};
  });
}
transmog_sdk_result *
transmog_sdk_application_session_detail(const transmog_sdk_application *app,
                                        const uint8_t *bytes,
                                        size_t length) noexcept {
  return invoke([&] {
    return app ? transmog::Result(
                     app->value.session_detail(input(bytes, length)))
               : transmog::Result{};
  });
}
transmog_sdk_result *transmog_sdk_application_release_traffic_view(
    const transmog_sdk_application *app, const uint8_t *bytes,
    size_t length) noexcept {
  return invoke([&] {
    return app ? transmog::Result(
                     app->value.release_traffic_view(input(bytes, length)))
               : transmog::Result{};
  });
}
transmog_sdk_result *transmog_sdk_default_proxy_start_request() noexcept {
  return invoke(
      [] { return transmog::Application::default_proxy_start_request(); });
}
transmog_sdk_result *transmog_sdk_default_session_query_input() noexcept {
  return invoke(
      [] { return transmog::Application::default_session_query_input(); });
}
transmog_sdk_body_page *transmog_sdk_application_read_body_range(
    const transmog_sdk_application *app, const uint8_t *session,
    size_t session_length, const uint8_t *boundary, size_t boundary_length,
    uint64_t offset, size_t max_bytes) noexcept {
  try {
    auto *page = new (std::nothrow) transmog_sdk_body_page{};
    if (page && app)
      page->value = app->value.read_body_range(
          {input(session, session_length), input(boundary, boundary_length),
           offset, max_bytes});
    return page;
  } catch (...) {
    return nullptr;
  }
}
int32_t
transmog_sdk_body_page_succeeded(const transmog_sdk_body_page *page) noexcept {
  return page && page->value.succeeded() ? 1 : 0;
}
transmog_sdk_error
transmog_sdk_body_page_error(const transmog_sdk_body_page *page) noexcept {
  const auto error = page ? page->value.error() : transmog::BodyPage{}.error();
  return {static_cast<uint32_t>(error.category), bytes(error.message),
          error.retryable ? 1u : 0u};
}
transmog_sdk_body_page_view
transmog_sdk_body_page_get_view(const transmog_sdk_body_page *page) noexcept {
  if (!page || !page->value.succeeded())
    return {};
  const auto value = page->value.view();
  const auto &meta = value.metadata;
  return {{value.bytes.data, value.bytes.size},
          value.offset,
          value.total_bytes,
          value.read_limit,
          static_cast<uint32_t>(meta.availability),
          meta.length_known ? 1u : 0u,
          meta.observed_bytes,
          meta.retained_bytes,
          meta.wire_body_bytes ? 1u : 0u,
          meta.wire_body_bytes.value_or(0),
          bytes(meta.exchange_id),
          bytes(meta.boundary),
          optional_text(meta.media_type),
          optional_text(meta.charset),
          optional_text(meta.sha256),
          optional_text(meta.reason),
          meta.content_coding_count};
}
transmog_sdk_bytes
transmog_sdk_body_page_content_coding(const transmog_sdk_body_page *page,
                                      size_t index) noexcept {
  return page ? bytes(page->value.content_coding(index)) : transmog_sdk_bytes{};
}
void transmog_sdk_body_page_free(transmog_sdk_body_page *page) noexcept {
  delete page;
}
transmog_sdk_result *transmog_sdk_application_traffic_view_rows(
    const transmog_sdk_application *app, const uint8_t *bytes, size_t length,
    size_t offset, size_t limit) noexcept {
  return invoke([&] {
    return app ? transmog::Result(app->value.traffic_view_rows(
                     input(bytes, length), offset, limit))
               : transmog::Result{};
  });
}
transmog_sdk_session_updates *
transmog_sdk_application_subscribe_session_updates(
    const transmog_sdk_application *app,
    transmog_sdk_session_update_callback callback, void *context) noexcept {
  try {
    auto *updates = new (std::nothrow) transmog_sdk_session_updates{};
    if (updates && app)
      updates->value = app->value.subscribe_session_updates(callback, context);
    return updates;
  } catch (...) {
    return nullptr;
  }
}
int32_t transmog_sdk_session_updates_valid(
    const transmog_sdk_session_updates *updates) noexcept {
  return updates && updates->value.valid() ? 1 : 0;
}
transmog_sdk_result *transmog_sdk_session_updates_status(
    const transmog_sdk_session_updates *updates) noexcept {
  return invoke(
      [&] { return updates ? updates->value.status() : transmog::Result{}; });
}
void transmog_sdk_session_updates_free(
    transmog_sdk_session_updates *updates) noexcept {
  delete updates;
}
int32_t
transmog_sdk_result_succeeded(const transmog_sdk_result *result) noexcept {
  return result && result->value.succeeded() ? 1 : 0;
}
const transmog_sdk_app_status *
transmog_sdk_result_as_app_status(const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::AppStatus>() : nullptr;
}
const transmog_sdk_product_state *transmog_sdk_result_as_product_state(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::ProductState>() : nullptr;
}
const transmog_sdk_proxy_start_request *
transmog_sdk_result_as_proxy_start_request(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::ProxyStartRequest>()
                : nullptr;
}
const transmog_sdk_session_query_input *
transmog_sdk_result_as_session_query_input(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::SessionQueryInput>()
                : nullptr;
}
const transmog_sdk_session_page *transmog_sdk_result_as_session_page(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::SessionPage>() : nullptr;
}
const transmog_sdk_traffic_view *transmog_sdk_result_as_traffic_view(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::TrafficView>() : nullptr;
}
const transmog_sdk_session_summary *transmog_sdk_result_as_session_summary(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::SessionSummary>()
                : nullptr;
}
const transmog_sdk_session_detail *transmog_sdk_result_as_session_detail(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::SessionDetail>()
                : nullptr;
}
const transmog_sdk_session_hint *transmog_sdk_result_as_session_hint(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::SessionHint>() : nullptr;
}
const transmog_sdk_app_error *
transmog_sdk_result_as_app_error(const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::AppError>() : nullptr;
}
const transmog_sdk_slice_session_summary *transmog_sdk_result_as_session_rows(
    const transmog_sdk_result *result) noexcept {
  return result ? result->value.get<transmog::models::SliceSessionSummary>()
                : nullptr;
}
transmog_sdk_error
transmog_sdk_result_error(const transmog_sdk_result *result) noexcept {
  const auto error =
      result ? result->value.error() : transmog::Result{}.error();
  return {static_cast<uint32_t>(error.category), bytes(error.message),
          error.retryable ? 1u : 0u};
}
void transmog_sdk_result_free(transmog_sdk_result *result) noexcept {
  delete result;
}
}
