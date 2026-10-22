#pragma once
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
#include <string_view>
#endif

// Length-delimited UTF-8. A zero length permits a null pointer.
typedef struct transmog_sdk_text {
  const uint8_t *data;
  size_t size;
#ifdef __cplusplus
  std::string_view text() const noexcept {
    return size ? std::string_view(reinterpret_cast<const char *>(data), size)
                : std::string_view{};
  }
  static transmog_sdk_text borrow(std::string_view value) noexcept {
    return {reinterpret_cast<const uint8_t *>(value.data()), value.size()};
  }
#endif
} transmog_sdk_text;
typedef struct transmog_sdk_text_entry {
  transmog_sdk_text key;
  transmog_sdk_text value;
} transmog_sdk_text_entry;
