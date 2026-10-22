#include "sdk_generated.hpp"
#include "transmog/transmog.hpp"
#include <new>
#include <utility>

namespace transmog {
namespace {
template <std::size_t N> Bytes literal(const char (&text)[N]) noexcept {
  return {reinterpret_cast<const std::uint8_t *>(text), N - 1};
}
} // namespace
Result::Result() noexcept : handle_(nullptr), failure_(Status::no_result) {}
Result::Result(std::uint8_t *handle, Status failure) noexcept
    : handle_(handle), failure_(failure) {}
Result::~Result() noexcept {
  if (handle_)
    transmog_sdk_rust::release(handle_);
}
Result::Result(Result &&other) noexcept
    : handle_(std::exchange(other.handle_, nullptr)),
      failure_(std::exchange(other.failure_, Status::no_result)) {}
Result &Result::operator=(Result &&other) noexcept {
  if (this != &other) {
    if (handle_)
      transmog_sdk_rust::release(handle_);
    handle_ = std::exchange(other.handle_, nullptr);
    failure_ = std::exchange(other.failure_, Status::no_result);
  }
  return *this;
}
Status Result::status() const noexcept {
  return handle_ ? (transmog_sdk_rust::succeeded(handle_) == 1
                        ? Status::success
                        : Status::operation_failed)
                 : failure_;
}
bool Result::succeeded() const noexcept { return status() == Status::success; }
Bytes Result::json() const noexcept {
  return handle_ ? Bytes{transmog_sdk_rust::result_json(handle_),
                         transmog_sdk_rust::result_json_length(handle_)}
                 : Bytes{nullptr, 0};
}
Bytes Result::error_json() const noexcept {
  if (handle_)
    return {transmog_sdk_rust::result_error(handle_),
            transmog_sdk_rust::result_error_length(handle_)};
  switch (failure_) {
  case Status::out_of_memory:
    return literal(
        R"({"category":"internal","message":"SDK allocation failed","retryable":false})");
  case Status::internal_error:
    return literal(
        R"({"category":"internal","message":"SDK internal error","retryable":false})");
  default:
    return literal(
        R"({"category":"invalid-input","message":"Empty or moved-from SDK handle","retryable":false})");
  }
}
Application::Application() noexcept
    : handle_(nullptr), failure_(Status::no_result) {}
Application::Application(std::uint8_t *handle, Status failure) noexcept
    : handle_(handle), failure_(failure) {}
Application::~Application() noexcept {
  if (handle_)
    transmog_sdk_rust::application_release(handle_);
}
Application::Application(Application &&other) noexcept
    : handle_(std::exchange(other.handle_, nullptr)),
      failure_(std::exchange(other.failure_, Status::no_result)) {}
Application &Application::operator=(Application &&other) noexcept {
  if (this != &other) {
    if (handle_)
      transmog_sdk_rust::application_release(handle_);
    handle_ = std::exchange(other.handle_, nullptr);
    failure_ = std::exchange(other.failure_, Status::no_result);
  }
  return *this;
}
Application Application::create() noexcept {
  try {
    return Application(transmog_sdk_rust::application_create(),
                       Status::out_of_memory);
  } catch (const std::bad_alloc &) {
    return Application(nullptr, Status::out_of_memory);
  } catch (...) {
    return Application(nullptr, Status::internal_error);
  }
}
Application
Application::create_with_body_store(std::string_view path) noexcept {
  try {
    return Application(
        transmog_sdk_rust::application_create_with_body_store(
            reinterpret_cast<const std::uint8_t *>(path.data()), path.size()),
        Status::out_of_memory);
  } catch (const std::bad_alloc &) {
    return Application(nullptr, Status::out_of_memory);
  } catch (...) {
    return Application(nullptr, Status::internal_error);
  }
}
bool Application::valid() const noexcept {
  return handle_ && transmog_sdk_rust::application_valid(handle_) == 1;
}
#define TRANSMOG_INVOKE(expression)                                            \
  if (!handle_)                                                                \
    return Result(nullptr, failure_);                                          \
  try {                                                                        \
    return Result(expression, Status::out_of_memory);                          \
  } catch (const std::bad_alloc &) {                                           \
    return Result(nullptr, Status::out_of_memory);                             \
  } catch (...) {                                                              \
    return Result(nullptr, Status::internal_error);                            \
  }
#define TRANSMOG_OPERATION(name, qualifier)                                    \
  Result Application::name() qualifier noexcept {                              \
    TRANSMOG_INVOKE(transmog_sdk_rust::name(handle_))                          \
  }
TRANSMOG_OPERATION(status, const)
TRANSMOG_OPERATION(product_state, const)
TRANSMOG_OPERATION(stop_proxy, )
TRANSMOG_OPERATION(shutdown, )
#undef TRANSMOG_OPERATION
#define TRANSMOG_INPUT_OPERATION(name, qualifier)                              \
  Result Application::name(std::string_view input) qualifier noexcept {        \
    TRANSMOG_INVOKE(transmog_sdk_rust::name(                                   \
        handle_, reinterpret_cast<const std::uint8_t *>(input.data()),         \
        input.size()))                                                         \
  }
TRANSMOG_INPUT_OPERATION(save_product_state, )
TRANSMOG_INPUT_OPERATION(start_proxy, )
TRANSMOG_INPUT_OPERATION(query_sessions, const)
TRANSMOG_INPUT_OPERATION(query_traffic_view, const)
TRANSMOG_INPUT_OPERATION(release_traffic_view, )
TRANSMOG_INPUT_OPERATION(session_detail, const)
TRANSMOG_INPUT_OPERATION(read_body_range, const)
#undef TRANSMOG_INPUT_OPERATION
Result Application::traffic_view_rows(std::string_view id, std::size_t offset,
                                      std::size_t limit) const noexcept {
  TRANSMOG_INVOKE(transmog_sdk_rust::traffic_view_rows(
      handle_, reinterpret_cast<const std::uint8_t *>(id.data()), id.size(),
      offset, limit))
}
#undef TRANSMOG_INVOKE

struct SessionUpdates::State {
  std::uint8_t *handle;
  SessionUpdateCallback callback;
  void *context;
};
SessionUpdates::SessionUpdates() noexcept
    : state_(nullptr), failure_(Status::no_result) {}
SessionUpdates::SessionUpdates(State *state, Status failure) noexcept
    : state_(state), failure_(failure) {}
void SessionUpdates::reset() noexcept {
  if (state_) {
    if (state_->handle)
      transmog_sdk_rust::session_updates_release(state_->handle);
    delete state_;
    state_ = nullptr;
  }
}
SessionUpdates::~SessionUpdates() noexcept { reset(); }
SessionUpdates::SessionUpdates(SessionUpdates &&other) noexcept
    : state_(std::exchange(other.state_, nullptr)),
      failure_(std::exchange(other.failure_, Status::no_result)) {}
SessionUpdates &SessionUpdates::operator=(SessionUpdates &&other) noexcept {
  if (this != &other) {
    reset();
    state_ = std::exchange(other.state_, nullptr);
    failure_ = std::exchange(other.failure_, Status::no_result);
  }
  return *this;
}
bool SessionUpdates::valid() const noexcept {
  return state_ && state_->handle &&
         transmog_sdk_rust::session_updates_valid(state_->handle) == 1;
}
Result SessionUpdates::status() const noexcept {
  if (!state_ || !state_->handle)
    return Result(nullptr, failure_);
  try {
    return Result(transmog_sdk_rust::session_updates_status(state_->handle),
                  Status::out_of_memory);
  } catch (const std::bad_alloc &) {
    return Result(nullptr, Status::out_of_memory);
  } catch (...) {
    return Result(nullptr, Status::internal_error);
  }
}
void SessionUpdates::deliver(std::uintptr_t address, const std::uint8_t *bytes,
                             std::size_t length) noexcept {
  const auto *state = reinterpret_cast<State *>(address);
  const auto callback = state->callback;
  auto *context = state->context;
  try {
    callback(context, bytes, length);
  } catch (...) {
    // Never unwind into Rust; context may have been released by the callback.
  }
}
SessionUpdates
Application::subscribe_session_updates(SessionUpdateCallback callback,
                                       void *context) const noexcept {
  if (!handle_)
    return SessionUpdates(nullptr, failure_);
  SessionUpdates::State *state = nullptr;
  try {
    state =
        new (std::nothrow) SessionUpdates::State{nullptr, callback, context};
    if (!state)
      return SessionUpdates(nullptr, Status::out_of_memory);
    state->handle = transmog_sdk_rust::subscribe_session_updates(
        handle_,
        callback ? reinterpret_cast<std::uintptr_t>(&SessionUpdates::deliver)
                 : 0,
        reinterpret_cast<std::uintptr_t>(state));
    return SessionUpdates(state, Status::out_of_memory);
  } catch (const std::bad_alloc &) {
    delete state;
    return SessionUpdates(nullptr, Status::out_of_memory);
  } catch (...) {
    delete state;
    return SessionUpdates(nullptr, Status::internal_error);
  }
}
std::uint32_t abi_version() noexcept { return 2; }
} // namespace transmog
