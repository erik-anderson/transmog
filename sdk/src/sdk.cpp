#include "sdk_generated.hpp"
#include "transmog/transmog.hpp"
#include <new>
#include <utility>

namespace transmog {
namespace {
bool compatible_layout() {
#include "model_layout.inc"
  return true;
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
const void *Result::model(std::uint32_t kind) const noexcept {
  return handle_ ? transmog_sdk_rust::result_model(handle_, kind) : nullptr;
}
Error Result::error() const noexcept {
  if (const auto *value = get<models::AppError>())
    return {static_cast<ErrorCategory>(value->category), value->message.text(),
            value->retryable != 0};
  if (succeeded())
    return {ErrorCategory::None, {}, false};
  switch (failure_) {
  case Status::out_of_memory:
    return {ErrorCategory::Internal, "SDK allocation failed", false};
  case Status::internal_error:
    return {ErrorCategory::Internal, "SDK internal error", false};
  default:
    return {ErrorCategory::InvalidInput, "Empty or moved-from SDK handle",
            false};
  }
}
BodyPage::BodyPage() noexcept : handle_(nullptr), failure_(Status::no_result) {}
BodyPage::BodyPage(std::uint8_t *handle, Status failure) noexcept
    : handle_(handle), failure_(failure) {}
BodyPage::~BodyPage() noexcept {
  if (handle_)
    transmog_sdk_rust::body_page_release(handle_);
}
BodyPage::BodyPage(BodyPage &&other) noexcept
    : handle_(std::exchange(other.handle_, nullptr)),
      failure_(std::exchange(other.failure_, Status::no_result)) {}
BodyPage &BodyPage::operator=(BodyPage &&other) noexcept {
  if (this != &other) {
    if (handle_)
      transmog_sdk_rust::body_page_release(handle_);
    handle_ = std::exchange(other.handle_, nullptr);
    failure_ = std::exchange(other.failure_, Status::no_result);
  }
  return *this;
}
Status BodyPage::status() const noexcept {
  return handle_ ? (transmog_sdk_rust::body_page_succeeded(handle_) == 1
                        ? Status::success
                        : Status::operation_failed)
                 : failure_;
}
bool BodyPage::succeeded() const noexcept {
  return status() == Status::success;
}
namespace {
// Private projection field IDs match engine/src/body.rs. Only primitive
// accessors cross Crubit, so no generated/Rust layout becomes public ABI.
Bytes body_data(std::uint8_t *handle, std::uint32_t field) noexcept {
  return {transmog_sdk_rust::body_page_data(handle, field),
          transmog_sdk_rust::body_page_data_length(handle, field)};
}
std::string_view body_text(std::uint8_t *handle, std::uint32_t field) noexcept {
  const auto bytes = body_data(handle, field);
  return bytes.data
             ? std::string_view(reinterpret_cast<const char *>(bytes.data),
                                bytes.size)
             : std::string_view{};
}
std::optional<std::string_view>
optional_body_text(std::uint8_t *handle, std::uint32_t field) noexcept {
  return transmog_sdk_rust::body_page_data(handle, field)
             ? std::optional{body_text(handle, field)}
             : std::nullopt;
}
} // namespace
Error BodyPage::error() const noexcept {
  if (handle_) {
    return {static_cast<ErrorCategory>(
                transmog_sdk_rust::body_page_number(handle_, 10)),
            body_text(handle_, 7),
            transmog_sdk_rust::body_page_number(handle_, 11) != 0};
  }
  switch (failure_) {
  case Status::out_of_memory:
    return {ErrorCategory::Internal, "SDK allocation failed", false};
  case Status::internal_error:
    return {ErrorCategory::Internal, "SDK internal error", false};
  default:
    return {ErrorCategory::InvalidInput, "Empty or moved-from SDK handle",
            false};
  }
}
BodyPageView BodyPage::view() const noexcept {
  if (!succeeded())
    return {};
  const auto number = [this](std::uint32_t field) noexcept {
    return transmog_sdk_rust::body_page_number(handle_, field);
  };
  return {body_data(handle_, 0),
          number(0),
          number(1),
          static_cast<std::size_t>(number(2)),
          {number(4) != 0, body_text(handle_, 1), body_text(handle_, 2),
           number(5), number(7) ? std::optional{number(8)} : std::nullopt,
           number(6), static_cast<BodyAvailability>(number(3)),
           optional_body_text(handle_, 3), optional_body_text(handle_, 4),
           static_cast<std::size_t>(number(9)), optional_body_text(handle_, 5),
           optional_body_text(handle_, 6)}};
}
std::string_view BodyPage::content_coding(std::size_t index) const noexcept {
  if (!succeeded())
    return {};
  const auto *data = transmog_sdk_rust::body_page_coding(handle_, index);
  return data ? std::string_view(
                    reinterpret_cast<const char *>(data),
                    transmog_sdk_rust::body_page_coding_length(handle_, index))
              : std::string_view{};
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
    if (!compatible_layout())
      return Application(nullptr, Status::internal_error);
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
    if (!compatible_layout())
      return Application(nullptr, Status::internal_error);
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
#define TRANSMOG_INVOKE(ResultType, expression)                                \
  if (!handle_)                                                                \
    return ResultType(nullptr, failure_);                                      \
  try {                                                                        \
    return ResultType(expression, Status::out_of_memory);                      \
  } catch (const std::bad_alloc &) {                                           \
    return ResultType(nullptr, Status::out_of_memory);                         \
  } catch (...) {                                                              \
    return ResultType(nullptr, Status::internal_error);                        \
  }
TypedResult<models::AppStatus> Application::status() const noexcept {
    TRANSMOG_INVOKE(
        TypedResult<models::AppStatus>,
        transmog_sdk_rust::status(
            handle_))} TypedResult<models::ProductState> Application::
    product_state() const noexcept {TRANSMOG_INVOKE(
        TypedResult<models::ProductState>,
        transmog_sdk_rust::product_state(
            handle_))} TypedResult<models::AppStatus> Application::
        stop_proxy() noexcept {
            TRANSMOG_INVOKE(TypedResult<models::AppStatus>,
                            transmog_sdk_rust::stop_proxy(handle_))} Result
    Application::shutdown() noexcept {TRANSMOG_INVOKE(
        Result, transmog_sdk_rust::shutdown(
                    handle_))} TypedResult<models::ProductState> Application::
        save_product_state(const models::ProductState &input) noexcept {
            TRANSMOG_INVOKE(
                TypedResult<models::ProductState>,
                transmog_sdk_rust::save_product_state(
                    handle_,
                    reinterpret_cast<const std::uint8_t *>(
                        &input)))} TypedResult<models::AppStatus> Application::
            start_proxy(const models::ProxyStartRequest &input) noexcept {
                TRANSMOG_INVOKE(
                    TypedResult<models::AppStatus>,
                    transmog_sdk_rust::start_proxy(
                        handle_,
                        reinterpret_cast<const std::uint8_t *>(
                            &input)))} TypedResult<models::
                                                       SessionPage> Application::
                query_sessions(const models::SessionQueryInput &input) const
    noexcept {TRANSMOG_INVOKE(
        TypedResult<models::SessionPage>,
        transmog_sdk_rust::query_sessions(
            handle_,
            reinterpret_cast<const std::uint8_t *>(
                &input)))} TypedResult<models::TrafficView> Application::
        query_traffic_view(const models::SessionQueryInput &input) const
    noexcept {TRANSMOG_INVOKE(
        TypedResult<models::TrafficView>,
        transmog_sdk_rust::query_traffic_view(
            handle_,
            reinterpret_cast<const std::uint8_t *>(
                &input)))} TypedResult<models::SessionDetail> Application::
        session_detail(std::string_view input) const noexcept {TRANSMOG_INVOKE(
            TypedResult<models::SessionDetail>,
            transmog_sdk_rust::session_detail(
                handle_, reinterpret_cast<const std::uint8_t *>(input.data()),
                input.size()))} Result
    Application::release_traffic_view(std::string_view input) noexcept {
        TRANSMOG_INVOKE(
            Result,
            transmog_sdk_rust::release_traffic_view(
                handle_, reinterpret_cast<const std::uint8_t *>(input.data()),
                input
                    .size()))} TypedResult<models::
                                               SliceSessionSummary> Application::
        traffic_view_rows(std::string_view id, std::size_t offset,
                          std::size_t limit) const noexcept {
            TRANSMOG_INVOKE(
                TypedResult<models::SliceSessionSummary>,
                transmog_sdk_rust::traffic_view_rows(
                    handle_, reinterpret_cast<const std::uint8_t *>(id.data()),
                    id.size(), offset, limit))}
#undef TRANSMOG_INVOKE
TypedResult<models::ProxyStartRequest> Application::
    default_proxy_start_request() noexcept {
  using Value = TypedResult<models::ProxyStartRequest>;
  try {
    if (!compatible_layout())
      return Value(nullptr, Status::internal_error);
    return Value(transmog_sdk_rust::default_proxy_start_request(),
                 Status::out_of_memory);
  } catch (const std::bad_alloc &) {
    return Value(nullptr, Status::out_of_memory);
  } catch (...) {
    return Value(nullptr, Status::internal_error);
  }
}
TypedResult<models::SessionQueryInput>
Application::default_session_query_input() noexcept {
  using Value = TypedResult<models::SessionQueryInput>;
  try {
    if (!compatible_layout())
      return Value(nullptr, Status::internal_error);
    return Value(transmog_sdk_rust::default_session_query_input(),
                 Status::out_of_memory);
  } catch (const std::bad_alloc &) {
    return Value(nullptr, Status::out_of_memory);
  } catch (...) {
    return Value(nullptr, Status::internal_error);
  }
}

BodyPage
Application::read_body_range(const BodyRangeRequest &request) const noexcept {
  if (!handle_)
    return BodyPage(nullptr, failure_);
  try {
    return BodyPage(
        transmog_sdk_rust::read_body_range(
            handle_,
            reinterpret_cast<const std::uint8_t *>(request.session_id.data()),
            request.session_id.size(),
            reinterpret_cast<const std::uint8_t *>(request.boundary.data()),
            request.boundary.size(), request.offset, request.max_bytes),
        Status::out_of_memory);
  } catch (const std::bad_alloc &) {
    return BodyPage(nullptr, Status::out_of_memory);
  } catch (...) {
    return BodyPage(nullptr, Status::internal_error);
  }
}

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
    if (bytes && length == sizeof(models::SessionHint))
      callback(context, reinterpret_cast<const models::SessionHint *>(bytes));
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
std::uint32_t abi_version() noexcept { return 4; }
} // namespace transmog
