#include <cstdio>
#include <cstring>
#include <transmog/transmog.h>
#include <transmog/transmog.hpp>
#include <utility>

extern "C" std::uint32_t transmog_c_header_version();
using transmog::Application;
namespace models = transmog::models;

static bool check(const transmog::Result &result) {
  if (result.succeeded())
    return true;
  const auto message = result.error().message;
  std::fprintf(stderr, "%.*s\n", static_cast<int>(message.size()),
               message.data());
  return false;
}

static int self_test(Application &app) {
  if (transmog::abi_version() != 4 || transmog_c_header_version() != 4)
    return 1;
  auto query = Application::default_session_query_input();
  auto start = Application::default_proxy_start_request();
  auto state = app.product_state();
  if (!check(query) || !check(start) || !check(state))
    return 2;
  if (!check(app.save_product_state(*state.get())) ||
      !check(app.query_sessions(*query.get())))
    return 3;
  auto invalid = *start.get();
  invalid.route = static_cast<models::ProxyRoute>(999);
  if (app.start_proxy(invalid).error().category !=
      transmog::ErrorCategory::InvalidInput)
    return 4;
  auto invalid_cache = Application::create_with_body_store({});
  if (invalid_cache.valid() || invalid_cache.status().succeeded() ||
      app.read_body_range({}).succeeded())
    return 5;
  auto view = app.query_traffic_view(*query.get());
  if (!check(view) || !check(app.traffic_view_rows(view->id.text(), 0, 1)) ||
      !check(app.release_traffic_view(view->id.text())))
    return 6;
  auto updates = app.subscribe_session_updates(
      [](void *, const models::SessionHint *) {}, nullptr);
  auto moved_updates = std::move(updates);
  if (updates.valid() || !moved_updates.valid() ||
      !check(moved_updates.status()))
    return 7;
  auto moved = std::move(app);
  if (app.valid() || !moved.valid() || !check(moved.shutdown()))
    return 8;
  auto *native = transmog_sdk_application_create();
  if (!native || !transmog_sdk_application_valid(native))
    return 9;
  auto *result = transmog_sdk_application_status(native);
  transmog_sdk_application_free(native);
  const auto *status = transmog_sdk_result_as_app_status(result);
  const bool success = transmog_sdk_result_succeeded(result) && status &&
                       status->lifecycle == models::AppLifecycle::Stopped &&
                       !transmog_sdk_result_as_product_state(result);
  transmog_sdk_result_free(result);
  return success ? 0 : 10;
}

int main(int argc, char **argv) {
  auto app = Application::create();
  if (!check(app.status()))
    return 1;
  if (argc == 2 && std::strcmp(argv[1], "--self-test") == 0)
    return self_test(app);
  if (argc != 1) {
    std::fprintf(stderr, "Usage: transmog_cpp_embedding [--self-test]\n");
    return 2;
  }
  auto state_owner = app.product_state();
  auto start_owner = Application::default_proxy_start_request();
  auto query_owner = Application::default_session_query_input();
  if (!check(state_owner) || !check(start_owner) || !check(query_owner))
    return 1;
  // Copy the generated view to change fields; its owner keeps borrowed text
  // alive.
  auto state = *state_owner.get();
  state.preferences.decrypt_https = 0;
  state.preferences.configure_system_proxy = 0;
  if (!check(app.save_product_state(state)))
    return 1;
  auto request = *start_owner.get();
  request.listen = models::Text::borrow("127.0.0.1:0");
  request.route = models::ProxyRoute::Http1;
  auto started = app.start_proxy(request);
  if (!check(started))
    return 1;
  const auto listener = started->listener.value.text();
  std::printf("Proxy listening on %.*s; send HTTP traffic, then press Enter.\n",
              static_cast<int>(listener.size()), listener.data());
  std::getchar();
  auto page = app.query_sessions(*query_owner.get());
  if (!check(page))
    return 1;
  for (const auto &row : page->sessions) {
    auto method = row.method.text(), path = row.path.text();
    std::printf("%.*s %.*s\n", static_cast<int>(method.size()), method.data(),
                static_cast<int>(path.size()), path.data());
  }
  return check(app.shutdown()) ? 0 : 1;
}
