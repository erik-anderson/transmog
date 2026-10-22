#pragma once
#include "transmog/export.h"
#include <cstddef>
#include <cstdint>
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
  // Existing BodyRangeRequest -> BodyBytePage, including raw bytesBase64,
  // exact decimal offsets, and retention/availability metadata. Pull bounded
  // pages to stream a body; a capturing prefix is not EOF. Reads do not pin
  // a body across calls, so eviction/loss remains visible to the consumer.
  Result read_body_range(std::string_view request_json) const noexcept;

private:
  Application(std::uint8_t *, Status) noexcept;
  std::uint8_t *handle_;
  Status failure_;
};
TRANSMOG_API std::uint32_t abi_version() noexcept;
} // namespace transmog
