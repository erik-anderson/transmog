// Transmog's ownership bridge to the unmodified BoringSSL PKI target.
// No X509_verify_cert path or alternate verifier exists here.
#include <openssl/pool.h>
#include <openssl/ssl.h>
#include <openssl/sha.h>

#include <chrono>
#include <ctime>
#include <memory>
#include <set>
#include <string>
#include <vector>

#include "pki/cert_issuer_source_static.h"
#include "pki/common_cert_errors.h"
#include "pki/encode_values.h"
#include "pki/general_names.h"
#include "pki/name_constraints.h"
#include "pki/path_builder.h"
#include "pki/simple_path_builder_delegate.h"
#include "pki/trust_store_in_memory.h"

// Compatibility name used by the unmodified Chromium blocklist snapshot.
namespace crypto::hash { constexpr size_t kSha256Size = SHA256_DIGEST_LENGTH; }
namespace {
using namespace bssl;
#include "cert_verify_proc_blocklist.inc"

DEFINE_CERT_ERROR_ID(kPublicLifetime, "Public certificate validity exceeds Chromium's lifetime policy");

std::string SpkiHash(const ParsedCertificate& cert) {
  uint8_t hash[SHA256_DIGEST_LENGTH];
  const auto spki = cert.tbs().spki_tlv;
  SHA256(spki.data(), spki.size(), hash);
  return std::string(reinterpret_cast<const char*>(hash), sizeof(hash));
}

void CheckStaticPolicy(CertPathBuilderResultPath* path) {
  if (!path->IsValid()) return;
  for (size_t i = 0; i < path->certs.size(); ++i) {
    const auto hash = SpkiHash(*path->certs[i]);
    for (const auto& blocked : kSPKIBlockList) {
      if (hash == std::string(reinterpret_cast<const char*>(blocked), sizeof(blocked))) {
        path->errors.GetErrorsForCert(i)->AddError(cert_errors::kCertificateRevoked);
      }
    }
  }
}

void CheckPublicLifetime(CertPathBuilderResultPath* path, bool known_root,
                         bool authority) {
  if (authority || !known_root || path->certs.empty()) return;
  int64_t start, expiry;
  const auto& validity = path->certs.front()->tbs();
  if (!der::GeneralizedTimeToPosixTime(validity.validity_not_before, &start) ||
      !der::GeneralizedTimeToPosixTime(validity.validity_not_after, &expiry) ||
      start > expiry) {
    path->errors.GetErrorsForCert(0)->AddError(kPublicLifetime);
    return;
  }
  const int64_t days = start >= 1868227200 ? 47 : start >= 1805068800 ? 100 : start >= 1773532800 ? 200 : 398;
  if (expiry - start > days * 86400)
    path->errors.GetErrorsForCert(0)->AddError(kPublicLifetime);
}

std::shared_ptr<const ParsedCertificate> Parse(const uint8_t* data,
                                               size_t len) {
  ParseCertificateOptions options;
  // net::x509_util::DefaultParseCertificateOptions in Chromium 154.
  options.allow_invalid_serial_numbers = true;
  return ParsedCertificate::Create(
      UniquePtr<CRYPTO_BUFFER>(CRYPTO_BUFFER_new(data, len, nullptr)), options,
      nullptr);
}

struct Policy : TrustStore {
  TrustStoreInMemory store;
  CertIssuerSourceStatic helpers;
  std::set<std::string, std::less<>> blocked;
  // TrustedPeople is exact leaf trust and is never searched for issuers.
  std::set<std::string, std::less<>> leaf_only;
  std::set<std::string, std::less<>> known_roots;
  bool KnownRoot(const ParsedCertificate* cert) const {
    return cert && known_roots.count(std::string(BytesAsStringView(cert->der_cert())));
  }

  CertificateTrust GetTrust(const ParsedCertificate* cert) override {
    if (blocked.count(std::string(BytesAsStringView(cert->der_cert())))) {
      return CertificateTrust::ForDistrusted();
    }
    return store.GetTrust(cert);
  }
  void SyncGetIssuersOf(const ParsedCertificate* cert,
                        ParsedCertificateList* issuers) override {
    ParsedCertificateList found;
    store.SyncGetIssuersOf(cert, &found);
    for (const auto& candidate : found) {
      if (!leaf_only.count(
              std::string(BytesAsStringView(candidate->der_cert())))) {
        issuers->push_back(candidate);
      }
    }
  }
  std::shared_ptr<const MTCAnchor> GetTrustedMTCIssuerOf(
      const ParsedCertificate*) override {
    return nullptr;
  }
};

struct Delegate : SimplePathBuilderDelegate {
  bool authority;
  const Policy* policy;
  std::chrono::steady_clock::time_point deadline;
  explicit Delegate(bool authority, const Policy* policy)
      : SimplePathBuilderDelegate(1024, DigestPolicy::kStrong),
        authority(authority),
        policy(policy),
        deadline(std::chrono::steady_clock::now() + std::chrono::seconds(60)) {}
  bool IsDeadlineExpired() override {
    return std::chrono::steady_clock::now() >= deadline;
  }
  void CheckPathAfterVerification(const CertPathBuilder&,
                                  CertPathBuilderResultPath* path) override {
    CheckStaticPolicy(path);
    if (!authority || path->certs.empty()) return;
    // Recovery selects an issuer of a future TLS leaf, rather than using that
    // issuer as an end entity. It must end at anchor trust, not TrustedPeople.
    if (!path->trust_anchor.CertTrust().IsTrustAnchor()) {
      path->errors.GetOtherErrors()->AddError(
          cert_errors::kDistrustedByTrustStore);
    }
    const auto& target = *path->certs.front();
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
      const auto& cert = *path->certs[i];
      const bool terminal = i + 1 == path->certs.size();
      if ((!terminal ||
           path->trust_anchor.CertTrust().enforce_anchor_constraints) &&
          cert.has_basic_constraints() &&
          cert.basic_constraints().has_path_len &&
          ca_count > cert.basic_constraints().path_len) {
        path->errors.GetErrorsForCert(i)->AddError(
            cert_errors::kMaxPathLengthViolated);
      }
      if (cert.normalized_subject() != cert.normalized_issuer()) ++ca_count;
    }
  }
};

struct Result {
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
}  // namespace

extern "C" {
size_t transmog_tls_peer_chain_len(const SSL* ssl) noexcept {
  const auto* chain = SSL_get0_peer_certificates(ssl);
  return chain ? sk_CRYPTO_BUFFER_num(chain) : 0;
}
const uint8_t* transmog_tls_peer_certificate(const SSL* ssl, size_t index,
                                             size_t* len) noexcept {
  const auto* chain = SSL_get0_peer_certificates(ssl);
  if (!chain || index >= sk_CRYPTO_BUFFER_num(chain)) {
    *len = 0;
    return nullptr;
  }
  const auto* cert = sk_CRYPTO_BUFFER_value(chain, index);
  *len = CRYPTO_BUFFER_len(cert);
  return CRYPTO_BUFFER_data(cert);
}
bool transmog_pki_certificate_parseable(const uint8_t* data,
                                        size_t len) noexcept {
  try {
    return static_cast<bool>(Parse(data, len));
  } catch (...) {
    return false;
  }
}
void* transmog_pki_policy_new() noexcept {
  try {
    return new Policy();
  } catch (...) {
    return nullptr;
  }
}
void transmog_pki_policy_free(void* policy) noexcept {
  delete static_cast<Policy*>(policy);
}
bool transmog_pki_policy_known_root(void* pointer, const uint8_t* data, size_t len) noexcept {
  try {
    static_cast<Policy*>(pointer)->known_roots.emplace(reinterpret_cast<const char*>(data), len);
    return true;
  } catch (...) { return false; }
}
// Kind: helper=0, public anchor=1, ROOT=2, TrustedPeople=3, Disallowed=4.
// The input is copied; no borrowed Rust data survives this call.
int transmog_pki_policy_add(void* pointer, const uint8_t* data, size_t len,
                            int kind, bool expiry, bool constraints) noexcept {
  try {
    auto* policy = static_cast<Policy*>(pointer);
    if (kind == 4) {
      policy->blocked.emplace(reinterpret_cast<const char*>(data), len);
      return 1;
    }
    auto cert = Parse(data, len);
    // Chromium ignores malformed local store entries and issuer candidates.
    if (!cert) return 1;
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

void* transmog_pki_verify(void* pointer, const uint8_t* const* certificates,
                          const size_t* lengths, size_t count,
                          int64_t posix_time, bool authority) noexcept {
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
    if (const auto* names = target->subject_alt_names()) {
      for (auto name : names->dns_names) result->dns_names.emplace_back(name);
      for (auto ip : names->ip_addresses)
        result->ip_addresses.emplace_back(BytesAsStringView(ip));
    }
    result->parsed = true;
    der::GeneralizedTime time;
    if (!der::EncodePosixTimeAsGeneralizedTime(posix_time, &time))
      return nullptr;
    auto* policy = static_cast<Policy*>(pointer);
    CertIssuerSourceStatic peers;
    for (size_t i = 1; i < count; ++i) {
      if (auto cert = Parse(certificates[i], lengths[i])) peers.AddCert(cert);
    }
    Delegate delegate(authority, policy);
    CertPathBuilder builder(
        target, policy, &delegate, time, KeyPurpose::SERVER_AUTH,
        InitialExplicitPolicy::kFalse, {der::Input(kAnyPolicyOid)},
        InitialPolicyMappingInhibit::kFalse, InitialAnyPolicyInhibit::kFalse);
    builder.AddCertIssuerSource(&peers);
    builder.AddCertIssuerSource(&policy->helpers);
    builder.SetIterationLimit(20);
    auto built = builder.Run();
    // CertVerifyProc applies the public lifetime policy to the selected path,
    // after path building; it must not trigger a search for a private alternate.
    if (built.best_result_index < built.paths.size()) {
      auto* best = built.paths[built.best_result_index].get();
      CheckPublicLifetime(best, policy->KnownRoot(best->GetTrustedCert()), authority);
    }
    result->valid = built.HasValidPath();
    const auto error = built.GetBestPathVerifyError();
    result->code = static_cast<int>(error.Code());
    result->depth = static_cast<int>(error.Index());
    result->diagnostic = error.DiagnosticString();
    if (const auto* best = built.GetBestPathPossiblyInvalid()) {
      result->known_root = policy->KnownRoot(best->GetTrustedCert());
      for (const auto& cert : best->certs)
        result->path.emplace_back(BytesAsStringView(cert->der_cert()));
      for (const auto& cert : best->certs)
        result->spki_hashes.emplace_back(SpkiHash(*cert));
    }
    return result.release();
  } catch (...) {
    return nullptr;
  }
}
void transmog_pki_result_free(void* result) noexcept {
  delete static_cast<Result*>(result);
}
int transmog_pki_result_code(const void* result) noexcept {
  return static_cast<const Result*>(result)->code;
}
int transmog_pki_result_depth(const void* result) noexcept {
  return static_cast<const Result*>(result)->depth;
}
bool transmog_pki_result_valid(const void* result) noexcept {
  return static_cast<const Result*>(result)->valid;
}
bool transmog_pki_result_known_root(const void* result) noexcept {
  return static_cast<const Result*>(result)->known_root;
}
const uint8_t* transmog_pki_result_spki_hash(const void* pointer, size_t index) noexcept {
  const auto& hashes = static_cast<const Result*>(pointer)->spki_hashes;
  return index < hashes.size() ? reinterpret_cast<const uint8_t*>(hashes[index].data()) : nullptr;
}
bool transmog_pki_result_parsed(const void* result) noexcept {
  return static_cast<const Result*>(result)->parsed;
}
size_t transmog_pki_result_path_len(const void* result) noexcept {
  return static_cast<const Result*>(result)->path.size();
}
size_t transmog_pki_result_names_len(const void* pointer, bool ip) noexcept {
  const auto* result = static_cast<const Result*>(pointer);
  return ip ? result->ip_addresses.size() : result->dns_names.size();
}
const uint8_t* transmog_pki_result_name(const void* pointer, bool ip,
                                        size_t index, size_t* len) noexcept {
  const auto* result = static_cast<const Result*>(pointer);
  const auto& names = ip ? result->ip_addresses : result->dns_names;
  if (index >= names.size()) {
    *len = 0;
    return nullptr;
  }
  *len = names[index].size();
  return reinterpret_cast<const uint8_t*>(names[index].data());
}
const uint8_t* transmog_pki_result_bytes(const void* pointer, size_t index,
                                         size_t* len) noexcept {
  const auto* result = static_cast<const Result*>(pointer);
  const auto& bytes =
      index < result->path.size() ? result->path[index] : result->diagnostic;
  *len = bytes.size();
  return reinterpret_cast<const uint8_t*>(bytes.data());
}
}
