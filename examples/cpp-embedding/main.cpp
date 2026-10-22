#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <string>
#include <transmog/transmog.h>
#include <transmog/transmog.hpp>
#include <utility>
extern "C" std::uint32_t transmog_c_header_version();
static void write(transmog::Bytes value, FILE *output) {
  if (value.size)
    std::fwrite(value.data, 1, value.size, output);
  std::fputc('\n', output);
}
static bool show(transmog::Result result) {
  write(result.succeeded() ? result.json() : result.error_json(),
        result.succeeded() ? stdout : stderr);
  std::fflush(stdout);
  return result.succeeded();
}
static std::string read(const char *path) {
  std::ifstream file(path, std::ios::binary);
  if (!file)
    return {};
  std::string value;
  char byte;
  while (file.get(byte)) {
    if (value.size() == 256 * 1024)
      return {};
    value.push_back(byte);
  }
  return value;
}
int main(int argc, char **argv) {
  auto app = transmog::Application::create();
  if (!app.valid()) {
    show(app.status());
    return 1;
  }
  if (argc == 2 && std::strcmp(argv[1], "--product-state") == 0)
    return show(app.product_state()) ? 0 : 1;
  if (argc == 2 && std::strcmp(argv[1], "--self-test") == 0) {
    if (transmog::abi_version() != 2 || transmog_c_header_version() != 2)
      return 1;
    if (!app.status().succeeded() || !app.product_state().succeeded() ||
        !app.query_sessions("{}").succeeded())
      return 2;
    if (app.start_proxy("{}").succeeded())
      return 3;
    auto invalid_cache = transmog::Application::create_with_body_store({});
    if (invalid_cache.valid() || invalid_cache.status().succeeded() ||
        app.read_body_range("{}").succeeded())
      return 10;
    if (app.query_traffic_view("not-json").succeeded() ||
        app.traffic_view_rows("missing", 0, 1).succeeded() ||
        !app.release_traffic_view("missing").succeeded())
      return 7;
    auto updates = app.subscribe_session_updates(
        [](void *, const std::uint8_t *, std::size_t) {}, nullptr);
    if (!updates.valid() || !updates.status().succeeded())
      return 8;
    auto moved_updates = std::move(updates);
    if (updates.valid() || !moved_updates.valid())
      return 9;
    auto moved = std::move(app);
    if (app.valid() || !moved.valid() || !moved.shutdown().succeeded())
      return 4;
    auto *native = transmog_sdk_application_create();
    if (!native || !transmog_sdk_application_valid(native))
      return 5;
    auto *result = transmog_sdk_application_status(native);
    transmog_sdk_application_free(native);
    const auto success = transmog_sdk_result_succeeded(result);
    transmog_sdk_result_free(result);
    return success ? 0 : 6;
  }
  if (argc != 3) {
    std::fprintf(stderr, "Usage: transmog_cpp_embedding <product-state.json> "
                         "<proxy-start.json>\n"
                         "       transmog_cpp_embedding --product-state\n");
    return 2;
  }
  if (!show(app.save_product_state(read(argv[1]))) ||
      !show(app.start_proxy(read(argv[2]))))
    return 1;
  std::fprintf(stderr, "Send traffic through the printed listener; press Enter "
                       "to inspect and stop.\n");
  std::getchar();
  if (!show(app.query_sessions("{}")))
    return 1;
  if (!show(app.stop_proxy()))
    return 1;
  return show(app.shutdown()) ? 0 : 1;
}
