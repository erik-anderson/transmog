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
using transmog::Result;
using transmog::SessionUpdates;
using transmog::Status;
#define OWNER(Type)                                                            \
  static_assert(std::is_nothrow_default_constructible_v<Type>);                \
  static_assert(std::is_nothrow_move_constructible_v<Type>);                   \
  static_assert(std::is_nothrow_move_assignable_v<Type>);                      \
  static_assert(std::is_nothrow_destructible_v<Type>);                         \
  static_assert(!std::is_copy_constructible_v<Type>);
OWNER(Application)
OWNER(Result)
OWNER(SessionUpdates)
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
OP(read_body_range)
#undef OP
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
  require(fake::applications == 0 && fake::results == 0,
          "all owners released once");
  require(!transmog_sdk_application_valid(nullptr) &&
              !transmog_sdk_result_succeeded(nullptr),
          "null safe");
  transmog_sdk_application_free(nullptr);
  transmog_sdk_result_free(nullptr);
}
