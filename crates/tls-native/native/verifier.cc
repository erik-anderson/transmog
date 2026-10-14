// Transmog's ownership bridge to the unmodified BoringSSL PKI target.
// No X509_verify_cert path or alternate verifier exists here.
#include <openssl/pool.h>
#include <openssl/sha.h>
#include <openssl/ssl.h>
#include <openssl/pkcs7.h>
#include <openssl/bytestring.h>
#include <openssl/err.h>

#include <chrono>
#include <ctime>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <vector>

#include "pki/cert_issuer_source_static.h"
#include "pki/common_cert_errors.h"
#include "pki/encode_values.h"
#include "pki/general_names.h"
#include "pki/name_constraints.h"
#include "pki/path_builder.h"
#include "pki/pem.h"
#include "pki/simple_path_builder_delegate.h"
#include "pki/trust_store_in_memory.h"
#include "security.h"

// Compatibility name used by the unmodified Chromium blocklist snapshot.
namespace crypto::hash {
constexpr size_t kSha256Size = SHA256_DIGEST_LENGTH;
}
namespace {
using namespace bssl;
struct IssuerHooks {
  void *context;
  bool (*valid_url)(const uint8_t *, size_t);
  void *(*start)(void *, const uint8_t *const *, const size_t *, size_t, uint64_t);
  bool (*next)(void *, void (*)(void *, const uint8_t *, size_t), void *);
  void (*destroy)(void *);
  bool (*cancelled)(void *);
};
#include "cert_verify_proc_blocklist.inc"

DEFINE_CERT_ERROR_ID(
    kPublicLifetime,
    "Public certificate validity exceeds Chromium's lifetime policy");
DEFINE_CERT_ERROR_ID(
    kRootConstraints,
    "Path does not satisfy Chromium root metadata constraints");
DEFINE_CERT_ERROR_ID(kCtRequired,
                     "Chromium Certificate Transparency requirements not met");

struct RootConstraint {
  bool version;
  std::optional<int64_t> validity_not_after, validity_after, sct_not_after,
      sct_all_after;
  std::vector<std::string> dns;
  bool Satisfied(const ParsedCertificate &leaf,
                 const transmog_security::Policy *security, int64_t now,
                 const transmog_security::Evidence &evidence) const {
    if (!version)
      return false;
    if (validity_not_after || validity_after) {
      int64_t start;
      der::GeneralizedTime converted;
      if ((validity_not_after && !der::EncodePosixTimeAsGeneralizedTime(
                                     *validity_not_after, &converted)) ||
          (validity_after && !der::EncodePosixTimeAsGeneralizedTime(
                                 *validity_after, &converted)) ||
          !der::GeneralizedTimeToPosixTime(leaf.tbs().validity_not_before,
                                           &start) ||
          (validity_not_after && start > *validity_not_after) ||
          (validity_after && start <= *validity_after))
        return false;
    }
    if ((sct_not_after || sct_all_after) && !security)
      return false;
    if (security && !security->disabled && security->Timely(now)) {
      bool any = false, before = false;
      for (const auto &sct : evidence.scts) {
        if (sct.log->Disqualified(now))
          continue;
        any = true;
        // Compare in seconds without rounding away the millisecond boundary.
        if (sct_not_after && sct.timestamp / 1000 <= *sct_not_after &&
            (sct.timestamp / 1000 < *sct_not_after ||
             sct.timestamp % 1000 == 0))
          before = true;
        if (sct_all_after && (sct.timestamp / 1000 < *sct_all_after ||
                              (sct.timestamp / 1000 == *sct_all_after &&
                               sct.timestamp % 1000 == 0)))
          return false;
      }
      if ((sct_not_after && !before) || (sct_all_after && !any))
        return false;
    }
    if (!dns.empty()) {
      GeneralNames permitted;
      for (const auto &name : dns)
        permitted.dns_names.push_back(name);
      permitted.present_name_types = GENERAL_NAME_DNS_NAME;
      auto limits =
          NameConstraints::CreateFromPermittedSubtrees(std::move(permitted));
      CertErrors errors;
      limits->IsPermittedCert(leaf.normalized_subject(),
                              leaf.subject_alt_names(), &errors);
      if (errors.ContainsAnyErrorWithSeverity(CertError::SEVERITY_HIGH))
        return false;
    }
    return true;
  }
};

std::string SpkiHash(const ParsedCertificate &cert) {
  uint8_t hash[SHA256_DIGEST_LENGTH];
  const auto spki = cert.tbs().spki_tlv;
  SHA256(spki.data(), spki.size(), hash);
  return std::string(reinterpret_cast<const char *>(hash), sizeof(hash));
}

void CheckStaticPolicy(CertPathBuilderResultPath *path) {
  if (!path->IsValid())
    return;
  for (size_t i = 0; i < path->certs.size(); ++i) {
    const auto hash = SpkiHash(*path->certs[i]);
    for (const auto &blocked : kSPKIBlockList) {
      if (hash == std::string(reinterpret_cast<const char *>(blocked),
                              sizeof(blocked))) {
        path->errors.GetErrorsForCert(i)->AddError(
            cert_errors::kCertificateRevoked);
      }
    }
  }
}

void CheckPublicLifetime(CertPathBuilderResultPath *path, bool known_root,
                         bool authority) {
  if (authority || !known_root || path->certs.empty())
    return;
  int64_t start, expiry;
  const auto &validity = path->certs.front()->tbs();
  if (!der::GeneralizedTimeToPosixTime(validity.validity_not_before, &start) ||
      !der::GeneralizedTimeToPosixTime(validity.validity_not_after, &expiry) ||
      start > expiry) {
    path->errors.GetErrorsForCert(0)->AddError(kPublicLifetime);
    return;
  }
  const int64_t days = start >= 1868227200   ? 47
                       : start >= 1805068800 ? 100
                       : start >= 1773532800 ? 200
                                             : 398;
  if (expiry - start > days * 86400)
    path->errors.GetErrorsForCert(0)->AddError(kPublicLifetime);
}

std::shared_ptr<const ParsedCertificate> Parse(const uint8_t *data,
                                               size_t len) {
  ParseCertificateOptions options;
  // net::x509_util::DefaultParseCertificateOptions in Chromium 154.
  options.allow_invalid_serial_numbers = true;
  return ParsedCertificate::Create(
      UniquePtr<CRYPTO_BUFFER>(CRYPTO_BUFFER_new(data, len, nullptr)), options,
      nullptr);
}

struct Policy : TrustStore {
  std::unique_ptr<transmog_security::Policy> security;
  TrustStoreInMemory store;
  CertIssuerSourceStatic helpers;
  std::set<std::string, std::less<>> blocked;
  // TrustedPeople is exact leaf trust and is never searched for issuers.
  std::set<std::string, std::less<>> leaf_only;
  std::set<std::string, std::less<>> known_roots;
  std::set<std::string, std::less<>> local_roots;
  std::map<std::string, std::vector<RootConstraint>, std::less<>> constraints;
  bool KnownRoot(const ParsedCertificate *cert) const {
    return cert &&
           known_roots.count(std::string(BytesAsStringView(cert->der_cert())));
  }

  CertificateTrust GetTrust(const ParsedCertificate *cert) override {
    if (blocked.count(std::string(BytesAsStringView(cert->der_cert())))) {
      return CertificateTrust::ForDistrusted();
    }
    return store.GetTrust(cert);
  }
  void SyncGetIssuersOf(const ParsedCertificate *cert,
                        ParsedCertificateList *issuers) override {
    ParsedCertificateList found;
    store.SyncGetIssuersOf(cert, &found);
    for (const auto &candidate : found) {
      if (!leaf_only.count(
              std::string(BytesAsStringView(candidate->der_cert())))) {
        issuers->push_back(candidate);
      }
    }
  }
  std::shared_ptr<const MTCAnchor>
  GetTrustedMTCIssuerOf(const ParsedCertificate *) override {
    return nullptr;
  }
};

// The path builder alone decides when to fetch. Responses are untrusted issuers.
class NetworkIssuers : public CertIssuerSource {
 public:
  NetworkIssuers(const IssuerHooks *hooks,
                 std::chrono::steady_clock::time_point deadline)
      : hooks_(hooks), deadline_(deadline) {}
  void SyncGetIssuersOf(const ParsedCertificate *, ParsedCertificateList *) override {}
  void AsyncGetIssuersOf(const ParsedCertificate *cert,
                        std::unique_ptr<Request> *out) override {
    out->reset();
    if (!hooks_ || hooks_->cancelled(hooks_->context)) return;
    std::vector<const uint8_t *> urls;
    std::vector<size_t> lengths;
    for (auto uri : cert->ca_issuers_uris()) {
      auto *bytes = reinterpret_cast<const uint8_t *>(uri.data());
      if (hooks_->valid_url(bytes, uri.size())) {
        urls.push_back(bytes);
        lengths.push_back(uri.size());
        if (urls.size() == 5) break;
      }
    }
    auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(
        deadline_ - std::chrono::steady_clock::now()).count();
    if (urls.empty() || remaining <= 0) return;
    void *request = hooks_->start(hooks_->context, urls.data(), lengths.data(),
                                 urls.size(), std::min<int64_t>(10000, remaining));
    if (request) *out = std::make_unique<Pending>(hooks_, request);
  }
 private:
  class Pending : public Request {
   public:
    Pending(const IssuerHooks *hooks, void *request) : hooks_(hooks), request_(request) {}
    ~Pending() override { hooks_->destroy(request_); }
    void GetNext(ParsedCertificateList *issuers) override {
      while (!hooks_->cancelled(hooks_->context) &&
             hooks_->next(request_, ParseResponse, issuers)) {
        if (!issuers->empty()) return;
      }
    }
   private:
    static void ParseResponse(void *output, const uint8_t *bytes, size_t len) noexcept {
      try {
        if (len > 65536) return;
        auto *issuers = static_cast<ParsedCertificateList *>(output);
        if (auto cert = Parse(bytes, len)) {
          issuers->push_back(std::move(cert));
          return;
        }
        CBS cbs;
        CBS_init(&cbs, bytes, len);
        UniquePtr<STACK_OF(CRYPTO_BUFFER)> buffers(sk_CRYPTO_BUFFER_new_null());
        bool cms = buffers && PKCS7_get_raw_certificates(buffers.get(), &cbs, nullptr);
        // Format probing must not leave ASN.1 errors on the TLS thread in sync mode.
        ERR_clear_error();
        if (cms) {
          for (size_t i = 0; i < sk_CRYPTO_BUFFER_num(buffers.get()); ++i) {
            auto *buffer = sk_CRYPTO_BUFFER_value(buffers.get(), i);
            if (auto cert = Parse(CRYPTO_BUFFER_data(buffer), CRYPTO_BUFFER_len(buffer)))
              issuers->push_back(std::move(cert));
          }
          if (!issuers->empty()) return;
        }
        PEMTokenizer pem(std::string_view(reinterpret_cast<const char *>(bytes), len),
                         std::vector<std::string>{"CERTIFICATE"});
        if (pem.GetNext()) {
          if (auto cert = Parse(reinterpret_cast<const uint8_t *>(pem.data().data()),
                                pem.data().size()))
            issuers->push_back(std::move(cert));
        }
      } catch (...) {}
    }
    const IssuerHooks *hooks_;
    void *request_;
  };
  const IssuerHooks *hooks_;
  std::chrono::steady_clock::time_point deadline_;
};

struct Delegate : SimplePathBuilderDelegate {
  bool authority;
  const Policy *policy;
  int64_t now;
  std::string_view tls_scts, staple;
  std::chrono::steady_clock::time_point deadline;
  const IssuerHooks *hooks;
  explicit Delegate(bool authority, const Policy *policy, int64_t now,
                    std::string_view tls_scts, std::string_view staple,
                    const IssuerHooks *hooks)
      : SimplePathBuilderDelegate(1024, DigestPolicy::kStrong),
        authority(authority), policy(policy), now(now), tls_scts(tls_scts),
        staple(staple),
        deadline(std::chrono::steady_clock::now() + std::chrono::seconds(60)), hooks(hooks) {}
  bool IsDeadlineExpired() override {
    return std::chrono::steady_clock::now() >= deadline ||
           (hooks && hooks->cancelled(hooks->context));
  }
  void CheckPathAfterVerification(const CertPathBuilder &,
                                  CertPathBuilderResultPath *path) override {
    auto evidence = std::make_unique<transmog_security::Evidence>();
    auto *data = evidence.get();
    path->delegate_data = std::move(evidence);
    if (policy->security && !authority)
      transmog_security::Collect(*policy->security, path->certs, tls_scts, now,
                                 data);
    if (!path->certs.empty()) {
      const std::string root(BytesAsStringView(path->certs.back()->der_cert()));
      auto found = policy->constraints.find(root);
      if (!policy->local_roots.count(root) &&
          found != policy->constraints.end()) {
        bool satisfied = false;
        for (const auto &group : found->second) {
          // Recovery asks whether this CA can issue a future TLS leaf. Its
          // issuance date, names and absent SCTs are not that leaf's evidence.
          // Keep root version eligibility here; the fresh TLS retry checks
          // every leaf constraint before authorizing a connection.
          satisfied |= authority
                           ? group.version
                           : group.Satisfied(*path->certs.front(),
                                             policy->security.get(), now, *data);
        }
        if (!satisfied)
          path->errors.GetOtherErrors()->AddError(kRootConstraints);
      }
    }
    if (policy->security && path->IsValid()) {
      transmog_security::Revocation(*policy->security, path,
                                    authority ? std::string_view() : staple,
                                    now, data);
      if (!authority && data->crl != 3) {
        if (policy->security->disabled)
          data->ct = 2;
        else if (!policy->security->Timely(now))
          data->ct = 3;
        else if (!policy->KnownRoot(path->GetTrustedCert()))
          data->ct = 1;
        else if (transmog_security::Complies(*policy->security, *path->certs[0],
                                             now, *data))
          data->ct = 4;
        else {
          data->ct = 5;
          path->errors.GetErrorsForCert(0)->AddError(kCtRequired);
        }
      }
    } else
      CheckStaticPolicy(path);
    if (!authority || path->certs.empty())
      return;
    // Recovery selects an issuer of a future TLS leaf, rather than using that
    // issuer as an end entity. It must end at anchor trust, not TrustedPeople.
    if (!path->trust_anchor.CertTrust().IsTrustAnchor()) {
      path->errors.GetOtherErrors()->AddError(
          cert_errors::kDistrustedByTrustStore);
    }
    const auto &target = *path->certs.front();
    const bool target_is_anchor =
        path->certs.size() == 1 &&
        path->trust_anchor.CertTrust().IsTrustAnchor();
    if (!target_is_anchor && (!target.has_basic_constraints() ||
                              !target.basic_constraints().is_ca)) {
      path->errors.GetErrorsForCert(0)->AddError(
          cert_errors::kMissingBasicConstraints);
    }
    if (target.has_key_usage() &&
        !target.key_usage().AssertsBit(KEY_USAGE_BIT_KEY_CERT_SIGN)) {
      path->errors.GetErrorsForCert(0)->AddError(
          cert_errors::kKeyCertSignBitNotSet);
    }
    // RFC 5280 excludes the target from the CA count. An advertised recovery
    // CA will issue another leaf, so include it when checking every pathLen.
    size_t ca_count =
        target.normalized_subject() == target.normalized_issuer() ? 0 : 1;
    for (size_t i = 1; i < path->certs.size(); ++i) {
      const auto &cert = *path->certs[i];
      const bool terminal = i + 1 == path->certs.size();
      if ((!terminal ||
           path->trust_anchor.CertTrust().enforce_anchor_constraints) &&
          cert.has_basic_constraints() &&
          cert.basic_constraints().has_path_len &&
          ca_count > cert.basic_constraints().path_len) {
        path->errors.GetErrorsForCert(i)->AddError(
            cert_errors::kMaxPathLengthViolated);
      }
      if (cert.normalized_subject() != cert.normalized_issuer())
        ++ca_count;
    }
  }
};

struct Result {
  uint32_t ct = 0, crl = 0, ocsp = 0, valid_scts = 0, invalid_scts = 0;
  bool interception = false;
  bool valid = false;
  bool known_root = false;
  bool parsed = false;
  int code = 16;
  int depth = -1;
  std::string diagnostic;
  std::vector<std::string> path;
  std::vector<std::string> spki_hashes;
  std::vector<std::string> dns_names;
  std::vector<std::string> ip_addresses;
};
} // namespace

extern "C" {
const uint8_t *transmog_tls_security(const SSL *ssl, uint32_t kind,
                                     size_t *len) noexcept {
  const uint8_t *data = nullptr;
  if (kind == 0)
    SSL_get0_signed_cert_timestamp_list(ssl, &data, len);
  else
    SSL_get0_ocsp_response(ssl, &data, len);
  return data;
}
size_t transmog_tls_peer_chain_len(const SSL *ssl) noexcept {
  const auto *chain = SSL_get0_peer_certificates(ssl);
  return chain ? sk_CRYPTO_BUFFER_num(chain) : 0;
}
const uint8_t *transmog_tls_peer_certificate(const SSL *ssl, size_t index,
                                             size_t *len) noexcept {
  const auto *chain = SSL_get0_peer_certificates(ssl);
  if (!chain || index >= sk_CRYPTO_BUFFER_num(chain)) {
    *len = 0;
    return nullptr;
  }
  const auto *cert = sk_CRYPTO_BUFFER_value(chain, index);
  *len = CRYPTO_BUFFER_len(cert);
  return CRYPTO_BUFFER_data(cert);
}
bool transmog_pki_certificate_parseable(const uint8_t *data,
                                        size_t len) noexcept {
  try {
    return static_cast<bool>(Parse(data, len));
  } catch (...) {
    return false;
  }
}
void *transmog_pki_policy_new() noexcept {
  try {
    return new Policy();
  } catch (...) {
    return nullptr;
  }
}
void transmog_pki_policy_free(void *policy) noexcept {
  delete static_cast<Policy *>(policy);
}
bool transmog_pki_policy_security(void *pointer, const uint8_t *data,
                                  size_t len) noexcept {
  try {
    auto security = std::make_unique<transmog_security::Policy>();
    if (!security->Parse(data, len))
      return false;
    for (const auto &h : kSPKIBlockList)
      security->blocked.emplace(reinterpret_cast<const char *>(h), sizeof(h));
    for (const auto &h : kKnownInterceptionList)
      security->interception.emplace(reinterpret_cast<const char *>(h),
                                     sizeof(h));
    static_cast<Policy *>(pointer)->security = std::move(security);
    return true;
  } catch (...) {
    return false;
  }
}
bool transmog_pki_policy_known_root(void *pointer, const uint8_t *data,
                                    size_t len) noexcept {
  try {
    static_cast<Policy *>(pointer)->known_roots.emplace(
        reinterpret_cast<const char *>(data), len);
    return true;
  } catch (...) {
    return false;
  }
}
bool transmog_pki_policy_local_root(void *pointer, const uint8_t *data,
                                    size_t len) noexcept {
  try {
    static_cast<Policy *>(pointer)->local_roots.emplace(
        reinterpret_cast<const char *>(data), len);
    return true;
  } catch (...) {
    return false;
  }
}
size_t transmog_pki_policy_constraint(void *pointer, const uint8_t *der,
                                      size_t len, bool version, uint32_t flags,
                                      int64_t not_after, int64_t after,
                                      int64_t sct_not_after,
                                      int64_t sct_after) noexcept {
  try {
    RootConstraint constraint;
    constraint.version = version;
    if (flags & 1)
      constraint.validity_not_after = not_after;
    if (flags & 2)
      constraint.validity_after = after;
    if (flags & 4)
      constraint.sct_not_after = sct_not_after;
    if (flags & 8)
      constraint.sct_all_after = sct_after;
    auto &constraints = static_cast<Policy *>(pointer)->constraints[std::string(
        reinterpret_cast<const char *>(der), len)];
    constraints.push_back(std::move(constraint));
    return constraints.size() - 1;
  } catch (...) {
    return SIZE_MAX;
  }
}
bool transmog_pki_policy_constraint_dns(void *pointer, const uint8_t *der,
                                        size_t len, size_t index,
                                        const uint8_t *dns,
                                        size_t dns_len) noexcept {
  try {
    auto found = static_cast<Policy *>(pointer)->constraints.find(
        std::string(reinterpret_cast<const char *>(der), len));
    if (found == static_cast<Policy *>(pointer)->constraints.end() ||
        index >= found->second.size())
      return false;
    found->second[index].dns.emplace_back(reinterpret_cast<const char *>(dns),
                                          dns_len);
    return true;
  } catch (...) {
    return false;
  }
}
// Kind: helper=0, public anchor=1, ROOT=2, TrustedPeople=3, Disallowed=4.
// The input is copied; no borrowed Rust data survives this call.
int transmog_pki_policy_add(void *pointer, const uint8_t *data, size_t len,
                            int kind, bool expiry, bool constraints) noexcept {
  try {
    auto *policy = static_cast<Policy *>(pointer);
    if (kind == 4) {
      policy->blocked.emplace(reinterpret_cast<const char *>(data), len);
      return 1;
    }
    auto cert = Parse(data, len);
    // Chromium ignores malformed local store entries and issuer candidates.
    if (!cert)
      return 1;
    if (kind == 0) {
      policy->helpers.AddCert(cert);
      return 1;
    }
    auto trust = kind == 1   ? CertificateTrust::ForTrustAnchor()
                 : kind == 2 ? CertificateTrust::ForTrustAnchorOrLeaf()
                             : CertificateTrust::ForTrustedLeaf();
    trust = trust.WithEnforceAnchorExpiry(expiry)
                .WithEnforceAnchorConstraints(constraints)
                .WithRequireLeafSelfSigned(kind == 2 || kind == 3);
    if (kind == 3)
      policy->leaf_only.emplace(BytesAsStringView(cert->der_cert()));
    policy->store.AddCertificate(cert, trust);
    return 1;
  } catch (...) {
    return 0;
  }
}

void *transmog_pki_verify(void *pointer, const uint8_t *const *certificates,
                          const size_t *lengths, size_t count, int64_t time_ms,
                          bool authority, const uint8_t *scts, size_t scts_len,
                          const uint8_t *ocsp, size_t ocsp_len,
                          const IssuerHooks *hooks) noexcept {
  try {
    auto result = std::make_unique<Result>();
    if (count == 0) {
      result->diagnostic = "No peer certificate";
      return result.release();
    }
    auto target = Parse(certificates[0], lengths[0]);
    if (!target) {
      result->code = 10;
      result->depth = 0;
      result->diagnostic =
          "BoringSSL PKI could not parse the target certificate";
      return result.release();
    }
    if (const auto *names = target->subject_alt_names()) {
      for (auto name : names->dns_names)
        result->dns_names.emplace_back(name);
      for (auto ip : names->ip_addresses)
        result->ip_addresses.emplace_back(BytesAsStringView(ip));
    }
    result->parsed = true;
    der::GeneralizedTime time;
    if (!der::EncodePosixTimeAsGeneralizedTime(time_ms / 1000, &time))
      return nullptr;
    auto *policy = static_cast<Policy *>(pointer);
    CertIssuerSourceStatic peers;
    for (size_t i = 1; i < count; ++i) {
      if (auto cert = Parse(certificates[i], lengths[i]))
        peers.AddCert(cert);
    }
    Delegate delegate(
        authority, policy, time_ms,
        std::string_view(reinterpret_cast<const char *>(scts), scts_len),
        std::string_view(reinterpret_cast<const char *>(ocsp), ocsp_len), hooks);
    NetworkIssuers network(hooks, delegate.deadline);
    CertPathBuilder builder(
        target, policy, &delegate, time, KeyPurpose::SERVER_AUTH,
        InitialExplicitPolicy::kFalse, {der::Input(kAnyPolicyOid)},
        InitialPolicyMappingInhibit::kFalse, InitialAnyPolicyInhibit::kFalse);
    builder.AddCertIssuerSource(&peers);
    builder.AddCertIssuerSource(&policy->helpers);
    if (hooks && !authority) builder.AddCertIssuerSource(&network);
    builder.SetIterationLimit(20);
    auto built = builder.Run();
    // CertVerifyProc applies the public lifetime policy to the selected path,
    // after path building; it must not trigger a search for a private
    // alternate.
    if (built.best_result_index < built.paths.size()) {
      auto *best = built.paths[built.best_result_index].get();
      CheckPublicLifetime(best, policy->KnownRoot(best->GetTrustedCert()),
                          authority);
    }
    result->valid = built.HasValidPath();
    const auto error = built.GetBestPathVerifyError();
    result->code = static_cast<int>(error.Code());
    result->depth = static_cast<int>(error.Index());
    result->diagnostic = error.DiagnosticString();
    if (const auto *best = built.GetBestPathPossiblyInvalid()) {
      if (const auto *evidence =
              static_cast<const transmog_security::Evidence *>(
                  best->delegate_data.get())) {
        result->ct = evidence->ct;
        result->crl = evidence->crl;
        result->ocsp = evidence->ocsp;
        result->valid_scts = evidence->valid_scts;
        result->invalid_scts = evidence->invalid_scts;
        result->interception = evidence->interception;
      }
      result->known_root = policy->KnownRoot(best->GetTrustedCert());
      for (const auto &cert : best->certs)
        result->path.emplace_back(BytesAsStringView(cert->der_cert()));
      for (const auto &cert : best->certs)
        result->spki_hashes.emplace_back(SpkiHash(*cert));
    }
    return result.release();
  } catch (...) {
    return nullptr;
  }
}
void transmog_pki_result_free(void *result) noexcept {
  delete static_cast<Result *>(result);
}
int transmog_pki_result_code(const void *result) noexcept {
  return static_cast<const Result *>(result)->code;
}
int transmog_pki_result_depth(const void *result) noexcept {
  return static_cast<const Result *>(result)->depth;
}
bool transmog_pki_result_valid(const void *result) noexcept {
  return static_cast<const Result *>(result)->valid;
}
bool transmog_pki_result_known_root(const void *result) noexcept {
  return static_cast<const Result *>(result)->known_root;
}
uint32_t transmog_pki_result_security(const void *pointer,
                                      uint32_t field) noexcept {
  const auto *r = static_cast<const Result *>(pointer);
  switch (field) {
  case 0:
    return r->ct;
  case 1:
    return r->crl;
  case 2:
    return r->ocsp;
  case 3:
    return r->valid_scts;
  case 4:
    return r->invalid_scts;
  case 5:
    return r->interception;
  default:
    return 0;
  }
}
const uint8_t *transmog_pki_result_spki_hash(const void *pointer,
                                             size_t index) noexcept {
  const auto &hashes = static_cast<const Result *>(pointer)->spki_hashes;
  return index < hashes.size()
             ? reinterpret_cast<const uint8_t *>(hashes[index].data())
             : nullptr;
}
bool transmog_pki_result_parsed(const void *result) noexcept {
  return static_cast<const Result *>(result)->parsed;
}
size_t transmog_pki_result_path_len(const void *result) noexcept {
  return static_cast<const Result *>(result)->path.size();
}
size_t transmog_pki_result_names_len(const void *pointer, bool ip) noexcept {
  const auto *result = static_cast<const Result *>(pointer);
  return ip ? result->ip_addresses.size() : result->dns_names.size();
}
const uint8_t *transmog_pki_result_name(const void *pointer, bool ip,
                                        size_t index, size_t *len) noexcept {
  const auto *result = static_cast<const Result *>(pointer);
  const auto &names = ip ? result->ip_addresses : result->dns_names;
  if (index >= names.size()) {
    *len = 0;
    return nullptr;
  }
  *len = names[index].size();
  return reinterpret_cast<const uint8_t *>(names[index].data());
}
const uint8_t *transmog_pki_result_bytes(const void *pointer, size_t index,
                                         size_t *len) noexcept {
  const auto *result = static_cast<const Result *>(pointer);
  const auto &bytes =
      index < result->path.size() ? result->path[index] : result->diagnostic;
  *len = bytes.size();
  return reinterpret_cast<const uint8_t *>(bytes.data());
}
}
