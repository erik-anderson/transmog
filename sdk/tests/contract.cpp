#include "sdk_generated.hpp"
#include "transmog/transmog.h"
#include "transmog/transmog.hpp"
#include <cstdio>
#include <cstdlib>
#include <new>
#include <type_traits>
#include <utility>
static bool fail_allocation = false;
void *operator new(std::size_t size, const std::nothrow_t &) noexcept {
  if (fail_allocation)
    return nullptr;
  try {
    return ::operator new(size);
  } catch (...) {
    return nullptr;
  }
}
void operator delete(void *pointer, const std::nothrow_t &) noexcept {
  ::operator delete(pointer);
}
void require(bool condition, const char *message) {
  if (!condition) {
    std::fprintf(stderr, "%s\n", message);
    std::abort();
  }
}
using transmog::Application;
using transmog::BodyPage;
using transmog::Result;
using transmog::SessionUpdates;
using transmog::Status;
static_assert(
    static_cast<std::uint32_t>(transmog::BodyAvailability::complete) ==
    TRANSMOG_BODY_COMPLETE);
static_assert(
    static_cast<std::uint32_t>(transmog::ErrorCategory::invalid_input) ==
    TRANSMOG_ERROR_INVALID_INPUT);
#define OWNER(Type)                                                            \
  static_assert(std::is_nothrow_default_constructible_v<Type>);                \
  static_assert(std::is_nothrow_move_constructible_v<Type>);                   \
  static_assert(std::is_nothrow_move_assignable_v<Type>);                      \
  static_assert(std::is_nothrow_destructible_v<Type>);                         \
  static_assert(!std::is_copy_constructible_v<Type>);
OWNER(Application)
OWNER(Result)
OWNER(SessionUpdates)
OWNER(BodyPage)
#undef OWNER
static_assert(noexcept(Application::create()));
static_assert(noexcept(Application::create_with_body_store({})));
static_assert(noexcept(std::declval<Application &>().valid()));
#define OP(name)                                                               \
  static_assert(noexcept(std::declval<Application &>().name()));               \
  static_assert(noexcept(transmog_sdk_application_##name(nullptr)));
OP(status)
OP(product_state)
OP(stop_proxy)
OP(shutdown)
#undef OP
#define OP(name)                                                               \
  static_assert(noexcept(std::declval<Application &>().name({})));             \
  static_assert(noexcept(transmog_sdk_application_##name(nullptr, nullptr, 0)));
OP(save_product_state)
OP(start_proxy)
OP(query_sessions)
OP(query_traffic_view)
OP(release_traffic_view)
OP(session_detail)
#undef OP
static_assert(noexcept(std::declval<Application &>().read_body_range({})));
static_assert(noexcept(transmog_sdk_application_read_body_range(
    nullptr, nullptr, 0, nullptr, 0, 0, 0)));
static_assert(noexcept(std::declval<BodyPage &>().status()));
static_assert(noexcept(std::declval<BodyPage &>().succeeded()));
static_assert(noexcept(std::declval<BodyPage &>().error()));
static_assert(noexcept(std::declval<BodyPage &>().view()));
static_assert(noexcept(std::declval<BodyPage &>().content_coding(0)));
static_assert(noexcept(transmog_sdk_body_page_succeeded(nullptr)));
static_assert(noexcept(transmog_sdk_body_page_error(nullptr)));
static_assert(noexcept(transmog_sdk_body_page_get_view(nullptr)));
static_assert(noexcept(transmog_sdk_body_page_content_coding(nullptr, 0)));
static_assert(noexcept(transmog_sdk_body_page_free(nullptr)));
static_assert(noexcept(std::declval<Application &>().traffic_view_rows({}, 0,
                                                                       1)));
static_assert(noexcept(
    transmog_sdk_application_traffic_view_rows(nullptr, nullptr, 0, 0, 1)));
static_assert(noexcept(
    std::declval<Application &>().subscribe_session_updates(nullptr, nullptr)));
static_assert(noexcept(std::declval<SessionUpdates &>().valid()));
static_assert(noexcept(std::declval<SessionUpdates &>().status()));
static_assert(noexcept(transmog_sdk_application_subscribe_session_updates(
    nullptr, nullptr, nullptr)));
static_assert(noexcept(transmog_sdk_session_updates_valid(nullptr)));
static_assert(noexcept(transmog_sdk_session_updates_status(nullptr)));
static_assert(noexcept(transmog_sdk_session_updates_free(nullptr)));
#define BUFFER(name)                                                           \
  static_assert(noexcept(std::declval<Result &>().name()));                    \
  static_assert(noexcept(transmog_sdk_result_##name(nullptr)));                \
  static_assert(noexcept(transmog_sdk_result_##name##_length(nullptr)));
BUFFER(json)
BUFFER(error_json)
#undef BUFFER
static_assert(noexcept(std::declval<Result &>().status()));
static_assert(noexcept(std::declval<Result &>().succeeded()));
static_assert(noexcept(transmog_sdk_result_succeeded(nullptr)));
static_assert(noexcept(transmog_sdk_result_free(nullptr)));
static_assert(noexcept(transmog_sdk_application_create()));
static_assert(noexcept(transmog_sdk_application_create_with_body_store(nullptr,
                                                                       0)));
static_assert(noexcept(transmog_sdk_application_valid(nullptr)));
static_assert(noexcept(transmog_sdk_application_free(nullptr)));
static_assert(noexcept(transmog::abi_version()));
static_assert(noexcept(transmog_sdk_abi_version()));
int main() {
  namespace fake = transmog_sdk_rust;
  Result retained;
  {
    auto app = Application::create();
    auto updates = app.subscribe_session_updates(
        [](void *, const std::uint8_t *, std::size_t) { throw 42; }, nullptr);
    auto moved = std::move(updates);
    require(!updates.valid() && moved.valid() && moved.status().succeeded(),
            "subscription owns a stable callback context across moves");
    reinterpret_cast<void (*)(std::uintptr_t, const std::uint8_t *,
                              std::size_t)>(fake::update_callback)(
        fake::update_context, nullptr, 0);
    require(fake::subscriptions == 1, "callback exception contained");
  }
  require(fake::subscriptions == 0, "subscription released exactly once");
  {
    BodyPage retained_page;
    {
      auto app = Application::create();
      auto page = app.read_body_range({"exchange", "client-response", 0, 3});
      auto view = page.view();
      require(page.succeeded() &&
                  page.error().category == transmog::ErrorCategory::none &&
                  view.offset == 9007199254740994ULL &&
                  view.total_bytes == 9007199254741009ULL &&
                  view.bytes.size == 3 && view.bytes.data[1] == 255 &&
                  view.metadata.length_known &&
                  view.metadata.availability ==
                      transmog::BodyAvailability::complete &&
                  view.metadata.exchange_id == "exchange" &&
                  view.metadata.boundary == "client-response" &&
                  view.metadata.media_type == "application/octet-stream" &&
                  view.metadata.charset == "" && !view.metadata.reason &&
                  !view.metadata.sha256 &&
                  view.metadata.content_coding_count == 1 &&
                  page.content_coding(0) == "gzip" &&
                  page.content_coding(1).empty(),
              "typed pages preserve exact numbers, binary bytes, and optional "
              "metadata");
      retained_page = std::move(page);
      require(!page.succeeded() && page.view().bytes.data == nullptr,
              "moved-from typed page is safe");
      auto *alias = &retained_page;
      retained_page = std::move(*alias);
    }
    require(retained_page.view().bytes.data[1] == 255 &&
                fake::applications == 0 && fake::results == 1,
            "typed page outlives its application");
    auto app = Application::create();
    for (auto failure : {fake::Failure::allocation, fake::Failure::exception,
                         fake::Failure::unknown, fake::Failure::null_handle}) {
      fake::failure = failure;
      auto failed = app.read_body_range({});
      require(!failed.succeeded() &&
                  failed.error().category ==
                      transmog::ErrorCategory::internal &&
                  failed.view().bytes.data == nullptr,
              "typed read exceptions/failures stay inspectable");
    }
    fake::failure = fake::Failure::none;
  }
  require(fake::results == 0, "typed page released once");
  {
    auto app = Application::create();
    auto second = Application::create();
    require(fake::applications == 2, "two applications");
    second = std::move(app);
    require(fake::applications == 1 && second.valid() && !app.valid(),
            "application move assignment");
    auto moved = std::move(second);
    require(moved.valid() && !second.valid(), "application move construction");
    auto *alias = &moved;
    moved = std::move(*alias);
    auto first = moved.status();
    retained = moved.product_state();
    require(fake::results == 2, "two independent result owners");
    retained = std::move(first);
    require(fake::results == 1 && retained.succeeded(),
            "result move assignment releases prior owner");
    require(first.status() == Status::no_result, "moved-from result safe");
    auto third = std::move(retained);
    auto *result_alias = &third;
    third = std::move(*result_alias);
    retained = std::move(third);
    fake::failure = fake::Failure::allocation;
    require(moved.status().status() == Status::out_of_memory,
            "bad_alloc contained");
    fake::failure = fake::Failure::exception;
    require(moved.start_proxy("{}").status() == Status::internal_error,
            "standard exception contained");
    fake::failure = fake::Failure::unknown;
    require(moved.shutdown().status() == Status::internal_error,
            "unknown exception contained");
    fake::failure = fake::Failure::null_handle;
    require(moved.query_sessions("{}").status() == Status::out_of_memory,
            "null result handled");
    fake::failure = fake::Failure::none;
  }
  require(fake::applications == 0 && fake::results == 1 &&
              retained.json().size == 1,
          "result survives application destruction");
  retained = Result{};
  require(fake::results == 0, "all results released");
  fake::failure = fake::Failure::exception;
  auto invalid = Application::create();
  require(!invalid.valid() &&
              invalid.status().status() == Status::internal_error,
          "creation exception is an inspectable error");
  fake::failure = fake::Failure::none;
  fail_allocation = true;
  auto creations = fake::creations;
  require(!transmog_sdk_application_create() && fake::creations == creations,
          "allocate C application wrapper before starting executor");
  require(!transmog_sdk_application_create_with_body_store(nullptr, 0) &&
              fake::creations == creations,
          "allocate C application wrapper before enabling body storage");
  fail_allocation = false;
  auto *app = transmog_sdk_application_create();
  require(app && transmog_sdk_application_valid(app),
          "C application delegates to C++");
  fail_allocation = true;
  auto operations = fake::operations;
  require(!transmog_sdk_application_shutdown(app) &&
              fake::operations == operations,
          "allocate C result wrapper before side effects");
  require(!transmog_sdk_application_read_body_range(app, nullptr, 0, nullptr, 0,
                                                    0, 0) &&
              fake::operations == operations,
          "allocate typed C page before reading");
  require(!transmog_sdk_application_release_traffic_view(app, nullptr, 0) &&
              fake::operations == operations,
          "allocate C result wrapper before releasing view");
  require(!transmog_sdk_application_subscribe_session_updates(app, nullptr,
                                                              nullptr) &&
              fake::operations == operations,
          "allocate C subscription wrapper before starting delivery");
  fail_allocation = false;
  auto *result = transmog_sdk_application_status(app);
  transmog_sdk_application_free(app);
  require(transmog_sdk_result_succeeded(result),
          "C result independent of application");
  transmog_sdk_result_free(result);
  app = transmog_sdk_application_create();
  auto *page = transmog_sdk_application_read_body_range(app, nullptr, 0,
                                                        nullptr, 0, 0, 0);
  transmog_sdk_application_free(app);
  const auto view = transmog_sdk_body_page_get_view(page);
  require(transmog_sdk_body_page_succeeded(page) &&
              view.offset == 9007199254740994ULL && view.bytes.size == 3 &&
              view.bytes.data[1] == 255 && view.charset.data != nullptr &&
              view.charset.size == 0 && view.sha256.data == nullptr &&
              view.has_wire_body_bytes == 1 &&
              view.wire_body_bytes == 9007199254741009ULL &&
              transmog_sdk_body_page_content_coding(page, 0).size == 4,
          "C typed views preserve ownership, exact numbers, and null versus "
          "empty metadata");
  transmog_sdk_body_page_free(page);
  require(!transmog_sdk_body_page_succeeded(nullptr) &&
              transmog_sdk_body_page_get_view(nullptr).bytes.data == nullptr &&
              transmog_sdk_body_page_error(nullptr).category == 3,
          "typed null owners are safe");
  transmog_sdk_body_page_free(nullptr);
  require(fake::applications == 0 && fake::results == 0,
          "all owners released once");
  require(!transmog_sdk_application_valid(nullptr) &&
              !transmog_sdk_result_succeeded(nullptr),
          "null safe");
  transmog_sdk_application_free(nullptr);
  transmog_sdk_result_free(nullptr);
}
