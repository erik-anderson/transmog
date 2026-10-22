#pragma once
#include <cstddef>
#include <cstdint>
#include <new>
#include <stdexcept>
namespace transmog_sdk_rust {
enum class Failure { none, allocation, exception, unknown, null_handle };
inline Failure failure = Failure::none;
inline int applications = 0, results = 0, operations = 0, creations = 0;
inline int subscriptions = 0;
inline std::uintptr_t update_callback = 0, update_context = 0;
inline std::uint8_t storage = 0;
inline std::uint8_t *allocate() {
  switch (failure) {
  case Failure::allocation:
    throw std::bad_alloc();
  case Failure::exception:
    throw std::runtime_error("injected");
  case Failure::unknown:
    throw 42;
  case Failure::null_handle:
    return nullptr;
  default:
    return &storage;
  }
}
inline std::uint8_t *application_create() {
  ++creations;
  auto *result = allocate();
  if (result)
    ++applications;
  return result;
}
inline std::uint8_t *application_create_with_body_store(const std::uint8_t *,
                                                        std::size_t) {
  return application_create();
}
inline std::int32_t application_valid(std::uint8_t *) noexcept { return 1; }
inline void application_release(std::uint8_t *) noexcept { --applications; }
inline std::uint8_t *operation() {
  ++operations;
  auto *result = allocate();
  if (result)
    ++results;
  return result;
}
#define FAKE_OPERATION(name)                                                   \
  inline std::uint8_t *name(std::uint8_t *) { return operation(); }
FAKE_OPERATION(status)
FAKE_OPERATION(product_state)
FAKE_OPERATION(stop_proxy)
FAKE_OPERATION(shutdown)
FAKE_OPERATION(session_updates_status)
#undef FAKE_OPERATION
#define FAKE_INPUT(name)                                                       \
  inline std::uint8_t *name(std::uint8_t *, const std::uint8_t *,              \
                            std::size_t) {                                     \
    return operation();                                                        \
  }
FAKE_INPUT(save_product_state)
FAKE_INPUT(start_proxy)
FAKE_INPUT(query_sessions)
FAKE_INPUT(session_detail)
FAKE_INPUT(query_traffic_view)
FAKE_INPUT(release_traffic_view)
#undef FAKE_INPUT
inline std::uint8_t *read_body_range(std::uint8_t *, const std::uint8_t *,
                                     std::size_t, const std::uint8_t *,
                                     std::size_t, std::uint64_t, std::size_t) {
  return operation();
}
inline std::int32_t body_page_succeeded(std::uint8_t *) noexcept { return 1; }
inline void body_page_release(std::uint8_t *) noexcept { --results; }
inline std::uint64_t body_page_number(std::uint8_t *,
                                      std::uint32_t field) noexcept {
  switch (field) {
  case 0:
    return 9007199254740994ULL;
  case 1:
    return 9007199254741009ULL;
  case 2:
    return 65536;
  case 3:
    return 2;
  case 4:
    return 1;
  case 5:
  case 6:
  case 8:
    return 9007199254741009ULL;
  case 7:
  case 9:
    return 1;
  default:
    return 0;
  }
}
inline const std::uint8_t *body_page_data(std::uint8_t *,
                                          std::uint32_t field) noexcept {
  static const std::uint8_t binary[]{0, 255, 1};
  switch (field) {
  case 0:
    return binary;
  case 1:
    return reinterpret_cast<const std::uint8_t *>("exchange");
  case 2:
    return reinterpret_cast<const std::uint8_t *>("client-response");
  case 3:
    return reinterpret_cast<const std::uint8_t *>("application/octet-stream");
  case 4:
    return reinterpret_cast<const std::uint8_t *>("");
  default:
    return nullptr;
  }
}
inline std::size_t body_page_data_length(std::uint8_t *,
                                         std::uint32_t field) noexcept {
  switch (field) {
  case 0:
    return 3;
  case 1:
    return 8;
  case 2:
    return 15;
  case 3:
    return 24;
  default:
    return 0;
  }
}
inline const std::uint8_t *body_page_coding(std::uint8_t *,
                                            std::size_t index) noexcept {
  return index == 0 ? reinterpret_cast<const std::uint8_t *>("gzip") : nullptr;
}
inline std::size_t body_page_coding_length(std::uint8_t *,
                                           std::size_t index) noexcept {
  return index == 0 ? 4 : 0;
}
inline std::uint8_t *traffic_view_rows(std::uint8_t *, const std::uint8_t *,
                                       std::size_t, std::size_t, std::size_t) {
  return operation();
}
inline std::uint8_t *subscribe_session_updates(std::uint8_t *,
                                               std::uintptr_t callback,
                                               std::uintptr_t context) {
  ++operations;
  auto *result = allocate();
  if (result) {
    ++subscriptions;
    update_callback = callback;
    update_context = context;
  }
  return result;
}
inline std::int32_t session_updates_valid(std::uint8_t *) noexcept { return 1; }
inline void session_updates_release(std::uint8_t *) noexcept {
  --subscriptions;
}
inline void release(std::uint8_t *) noexcept { --results; }
inline std::int32_t succeeded(std::uint8_t *) noexcept { return 1; }
inline const std::uint8_t *result_json(std::uint8_t *) noexcept {
  return &storage;
}
inline std::size_t result_json_length(std::uint8_t *) noexcept { return 1; }
inline const std::uint8_t *result_error(std::uint8_t *) noexcept {
  return nullptr;
}
inline std::size_t result_error_length(std::uint8_t *) noexcept { return 0; }
} // namespace transmog_sdk_rust
