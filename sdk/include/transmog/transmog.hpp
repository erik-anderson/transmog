#pragma once
#include "transmog/export.h"
#include <cstddef>
#include <cstdint>
#include <optional>
#include <string_view>

namespace transmog {
enum class Status : std::uint32_t {
  success = 0,
  operation_failed = 1,
  out_of_memory = 2,
  internal_error = 3,
  no_result = 4
};
// Bytes borrowed from their owning result; never free this pointer.
struct Bytes {
  const std::uint8_t *data;
  std::size_t size;
};

enum class ErrorCategory : std::uint32_t {
  none = 0,
  password_required = 1,
  invalid_password = 2,
  invalid_input = 3,
  unsupported = 4,
  canceled = 5,
  conflict = 6,
  limit = 7,
  unavailable = 8,
  retryable = 9,
  internal = 10
};
struct Error {
  ErrorCategory category;
  std::string_view message;
  bool retryable;
};
enum class BodyAvailability : std::uint32_t {
  unavailable = 0,
  capturing = 1,
  complete = 2,
  truncated = 3,
  lost = 4,
  evicted = 5,
  disabled = 6,
  quota_omitted = 7
};
struct BodyRangeRequest {
  std::string_view session_id;
  std::string_view boundary;
  std::uint64_t offset = 0;
  std::size_t max_bytes = 64 * 1024;
};
// Text and byte views below are borrowed from their BodyPage owner. Optional
// text distinguishes absent values from present empty strings.
struct BodyMetadata {
  bool length_known;
  std::string_view exchange_id;
  std::string_view boundary;
  std::uint64_t observed_bytes;
  std::optional<std::uint64_t> wire_body_bytes;
  std::uint64_t retained_bytes;
  BodyAvailability availability;
  std::optional<std::string_view> media_type;
  std::optional<std::string_view> charset;
  std::size_t content_coding_count;
  std::optional<std::string_view> sha256;
  std::optional<std::string_view> reason;
};
struct BodyPageView {
  Bytes bytes;
  std::uint64_t offset;
  std::uint64_t total_bytes;
  std::size_t read_limit;
  BodyMetadata metadata;
};

// Owns a typed Application byte page or error, without JSON/base64. Views
// expire on destruction/replacement; the page can outlive its Application.
class TRANSMOG_API BodyPage final {
public:
  BodyPage() noexcept;
  ~BodyPage() noexcept;
  BodyPage(BodyPage &&) noexcept;
  BodyPage &operator=(BodyPage &&) noexcept;
  BodyPage(const BodyPage &) = delete;
  BodyPage &operator=(const BodyPage &) = delete;
  Status status() const noexcept;
  bool succeeded() const noexcept;
  Error error() const noexcept;
  BodyPageView view() const noexcept;
  std::string_view content_coding(std::size_t index) const noexcept;

private:
  friend class Application;
  BodyPage(std::uint8_t *, Status) noexcept;
  std::uint8_t *handle_;
  Status failure_;
};

class Application;
class SessionUpdates;
using SessionUpdateCallback = void (*)(void *context,
                                       const std::uint8_t *hint_json,
                                       std::size_t length);
// Owns the exact serialized Application output or AppError, with no Rust
// layouts.
class TRANSMOG_API Result final {
public:
  Result() noexcept;
  ~Result() noexcept;
  Result(Result &&) noexcept;
  Result &operator=(Result &&) noexcept;
  Result(const Result &) = delete;
  Result &operator=(const Result &) = delete;
  Status status() const noexcept;
  bool succeeded() const noexcept;
  Bytes json() const noexcept;
  Bytes error_json() const noexcept;

private:
  friend class Application;
  friend class SessionUpdates;
  Result(std::uint8_t *, Status) noexcept;
  std::uint8_t *handle_;
  Status failure_;
};

// Owns delivery of the application's lossy SessionHint JSON. Callbacks run on
// a native delivery thread; copy borrowed bytes before returning. Keep callback
// code/context alive until this owner is destroyed. Destruction stops delivery
// and waits for an in-flight callback, except when called by that callback.
// Callback exceptions are contained. The owner may outlive Application and
// retains its executor until released. valid/status report creation success.
class TRANSMOG_API SessionUpdates final {
public:
  SessionUpdates() noexcept;
  ~SessionUpdates() noexcept;
  SessionUpdates(SessionUpdates &&) noexcept;
  SessionUpdates &operator=(SessionUpdates &&) noexcept;
  SessionUpdates(const SessionUpdates &) = delete;
  SessionUpdates &operator=(const SessionUpdates &) = delete;
  bool valid() const noexcept;
  Result status() const noexcept;

private:
  friend class Application;
  struct State;
  SessionUpdates(State *, Status) noexcept;
  void reset() noexcept;
  static void deliver(std::uintptr_t, const std::uint8_t *,
                      std::size_t) noexcept;
  State *state_;
  Status failure_;
};

// Binding of transmog_app::Application, with one owned executor.
// Calls block; the proxy continues running on its native executor between
// calls. Serialize release/moves against calls. Result buffers have independent
// lifetimes.
class TRANSMOG_API Application final {
public:
  Application() noexcept;
  ~Application() noexcept;
  Application(Application &&) noexcept;
  Application &operator=(Application &&) noexcept;
  Application(const Application &) = delete;
  Application &operator=(const Application &) = delete;
  static Application create() noexcept;
  // Opt into BodyStoreConfig::product_default at a caller-owned cache
  // directory. The application uses the shared ProductState privacy/buffer
  // preferences.
  static Application
  create_with_body_store(std::string_view cache_directory) noexcept;
  bool valid() const noexcept;
  // status also reports a creation failure when valid() is false.
  Result status() const noexcept;
  Result product_state() const noexcept;
  Result save_product_state(std::string_view product_state_json) noexcept;
  // Uses ProxyStartRequest and caller-owned host setup (HostIntegration=None).
  Result start_proxy(std::string_view request_json) noexcept;
  // Existing application semantics: turn routing off and begin reversible
  // drain.
  Result stop_proxy() noexcept;
  // Existing application semantics: stop unfinished work and await final
  // shutdown.
  Result shutdown() noexcept;
  Result query_sessions(std::string_view query_json) const noexcept;
  Result query_traffic_view(std::string_view query_json) const noexcept;
  Result traffic_view_rows(std::string_view view_id, std::size_t offset,
                           std::size_t limit) const noexcept;
  // Releases the ordering snapshot; success contains JSON null.
  Result release_traffic_view(std::string_view view_id) noexcept;
  // Hints signal a re-query; they are not an authoritative row/event stream.
  SessionUpdates subscribe_session_updates(SessionUpdateCallback callback,
                                           void *context) const noexcept;
  Result session_detail(std::string_view session_id) const noexcept;
  // Same Application read, with numeric offsets, raw bytes, and typed metadata.
  // A capturing prefix is not EOF. Reads do not pin bodies across calls.
  BodyPage read_body_range(const BodyRangeRequest &request) const noexcept;

private:
  Application(std::uint8_t *, Status) noexcept;
  std::uint8_t *handle_;
  Status failure_;
};
TRANSMOG_API std::uint32_t abi_version() noexcept;
} // namespace transmog
