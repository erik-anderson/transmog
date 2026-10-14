// Transmog's CT/CRLSet policy adapter for Chromium's BoringSSL PKI paths.
// Policy semantics follow Chromium 154's MultiLogCTVerifier,
// ChromeCTPolicyEnforcer, CRLSet and revocation_checker.
#pragma once
#include "pki/ocsp.h"
#include "pki/parsed_certificate.h"
#include <algorithm>
#include <limits>
#include <map>
#include <openssl/bytestring.h>
#include <openssl/err.h>
#include <openssl/evp.h>
#include <openssl/sha.h>
#include <set>
#include <string>
#include <vector>

namespace transmog_security {
using namespace bssl;
struct ErrorScope {
  ~ErrorScope() {
    // Same error-queue hygiene as Chromium's OpenSSLErrStackTracer and PKI.
    ERR_clear_error();
  }
};
inline std::string Bytes(const CBS &c) {
  return std::string(reinterpret_cast<const char *>(CBS_data(&c)), CBS_len(&c));
}
inline std::string Hash(der::Input input) {
  uint8_t digest[32];
  SHA256(input.data(), input.size(), digest);
  return std::string(reinterpret_cast<const char *>(digest), sizeof(digest));
}
inline bool Blob(CBS *c, std::string *out) {
  CBS value;
  uint32_t length;
  if (!CBS_get_u32(c, &length) || !CBS_get_bytes(c, &value, length))
    return false;
  *out = Bytes(value);
  return true;
}
inline bool Hashes(CBS *c, std::set<std::string> *out) {
  uint32_t count;
  if (!CBS_get_u32(c, &count) || count > CBS_len(c) / 32)
    return false;
  for (uint32_t i = 0; i < count; ++i) {
    CBS h;
    if (!CBS_get_bytes(c, &h, 32))
      return false;
    out->insert(Bytes(h));
  }
  return true;
}
struct Log {
  UniquePtr<EVP_PKEY> key;
  uint32_t type;
  std::optional<int64_t> retired;
  std::vector<std::pair<std::string, int64_t>> operators;
  bool Disqualified(int64_t now) const { return retired && now >= *retired; }
  std::string Operator(int64_t timestamp) const {
    for (const auto &item : operators)
      if (timestamp >= item.second)
        return item.first;
    return operators.empty() ? std::string() : operators.back().first;
  }
};
struct Policy {
  bool disabled = false, require_rfc = false;
  int64_t timestamp = 0;
  uint64_t not_after = 0;
  std::map<std::string, Log> logs;
  std::set<std::string> blocked, interception;
  std::map<std::string, std::set<std::string>> subjects, issuers;
  bool Timely(int64_t now) const {
    // Avoid signed overflow for malformed/future component timestamps.
    return now < timestamp ||
           static_cast<uint64_t>(now) - static_cast<uint64_t>(timestamp) <
               70ULL * 86400000;
  }
  bool Parse(const uint8_t *data, size_t length) {
    ErrorScope errors;
    CBS c;
    CBS_init(&c, data, length);
    uint8_t flag;
    uint64_t time;
    uint32_t count;
    if (!CBS_get_u8(&c, &flag))
      return false;
    disabled = flag != 0;
    if (!CBS_get_u64(&c, &time) || !CBS_get_u8(&c, &flag) ||
        !CBS_get_u32(&c, &count) || count > 1024)
      return false;
    timestamp = static_cast<int64_t>(time);
    require_rfc = flag != 0;
    for (uint32_t i = 0; i < count; ++i) {
      CBS id;
      std::string key;
      Log log;
      if (!CBS_get_bytes(&c, &id, 32) || !Blob(&c, &key) ||
          !CBS_get_u32(&c, &log.type) || !CBS_get_u8(&c, &flag))
        return false;
      if (flag) {
        if (!CBS_get_u64(&c, &time))
          return false;
        log.retired = static_cast<int64_t>(time);
      }
      uint32_t n;
      if (!CBS_get_u32(&c, &n) || n > 64)
        return false;
      for (uint32_t j = 0; j < n; ++j) {
        std::string name;
        if (!Blob(&c, &name) || !CBS_get_u64(&c, &time))
          return false;
        log.operators.emplace_back(std::move(name), static_cast<int64_t>(time));
      }
      CBS spki;
      CBS_init(&spki, reinterpret_cast<const uint8_t *>(key.data()),
               key.size());
      log.key.reset(EVP_parse_public_key(&spki));
      if (!log.key || CBS_len(&spki) ||
          (EVP_PKEY_id(log.key.get()) != EVP_PKEY_EC &&
           EVP_PKEY_id(log.key.get()) != EVP_PKEY_RSA) ||
          (EVP_PKEY_id(log.key.get()) == EVP_PKEY_RSA &&
           EVP_PKEY_size(log.key.get()) < 256) ||
          Hash(der::Input(reinterpret_cast<const uint8_t *>(key.data()),
                          key.size())) != Bytes(id))
        continue;
      logs.insert_or_assign(Bytes(id), std::move(log));
    }
    if (!CBS_get_u64(&c, &not_after) || !Hashes(&c, &blocked) ||
        !Hashes(&c, &interception) || !CBS_get_u32(&c, &count) ||
        count > CBS_len(&c) / 36)
      return false;
    for (uint32_t i = 0; i < count; ++i) {
      CBS h;
      std::set<std::string> allowed;
      if (!CBS_get_bytes(&c, &h, 32) || !Hashes(&c, &allowed))
        return false;
      subjects[Bytes(h)] = std::move(allowed);
    }
    if (!CBS_get_u32(&c, &count) || count > CBS_len(&c) / 36)
      return false;
    for (uint32_t i = 0; i < count; ++i) {
      CBS h;
      uint32_t n;
      if (!CBS_get_bytes(&c, &h, 32) || !CBS_get_u32(&c, &n) ||
          n > CBS_len(&c) / 4)
        return false;
      auto &serials = issuers[Bytes(h)];
      for (uint32_t j = 0; j < n; ++j) {
        std::string serial;
        if (!Blob(&c, &serial))
          return false;
        serials.insert(std::move(serial));
      }
    }
    return CBS_len(&c) == 0;
  }
};
struct Sct {
  const Log *log;
  std::string id;
  int64_t timestamp;
  bool embedded;
  std::string extensions;
};
struct Evidence : CertPathBuilderDelegateData {
  // 0=not reached, 1=private, 2=disabled, 3=stale, 4=compliant,
  // 5=insufficient/diversity.
  uint32_t ct = 0, valid_scts = 0, invalid_scts = 0;
  // CRL: 0=not reached, 1=inconclusive, 2=covered, 3=revoked.
  uint32_t crl = 0;
  // OCSP: 0=not reached, 1=absent, 2=good, 3=revoked, 4=invalid/unknown.
  uint32_t ocsp = 0;
  bool interception = false;
  std::vector<Sct> scts;
};
inline bool Skip(CBS *c, unsigned tag) {
  CBS value;
  return !CBS_peek_asn1_tag(c, tag) || CBS_get_asn1(c, &value, tag);
}
inline bool EmbeddedEntry(const ParsedCertificate &leaf,
                          const ParsedCertificate &issuer, std::string *list,
                          std::string *entry) {
  constexpr uint8_t oid[] = {0x2b, 0x06, 0x01, 0x04, 0x01,
                             0xd6, 0x79, 0x02, 0x04, 0x02};
  ParsedExtension extension;
  if (!leaf.GetExtension(der::Input(oid), &extension))
    return false;
  CBS encoded, decoded;
  CBS_init(&encoded, extension.value.data(), extension.value.size());
  if (!CBS_get_asn1(&encoded, &decoded, CBS_ASN1_OCTETSTRING) ||
      CBS_len(&encoded))
    return false;
  *list = Bytes(decoded);
  CBS raw, body;
  const auto tbs = leaf.tbs_certificate_tlv();
  CBS_init(&raw, tbs.data(), tbs.size());
  if (!CBS_get_asn1(&raw, &body, CBS_ASN1_SEQUENCE) || CBS_len(&raw))
    return false;
  const CBS original = body;
  if (!Skip(&body, CBS_ASN1_CONTEXT_SPECIFIC | CBS_ASN1_CONSTRUCTED | 0))
    return false;
  for (size_t i = 0; i < 6; ++i)
    if (!CBS_get_any_asn1_element(&body, nullptr, nullptr, nullptr))
      return false;
  if (!Skip(&body, CBS_ASN1_CONTEXT_SPECIFIC | 1) ||
      !Skip(&body, CBS_ASN1_CONTEXT_SPECIFIC | 2))
    return false;
  const size_t prefix = CBS_data(&body) - CBS_data(&original);
  CBS wrapper, extensions;
  constexpr unsigned tag = CBS_ASN1_CONTEXT_SPECIFIC | CBS_ASN1_CONSTRUCTED | 3;
  if (!CBS_get_asn1(&body, &wrapper, tag) || CBS_len(&body) ||
      !CBS_get_asn1(&wrapper, &extensions, CBS_ASN1_SEQUENCE) ||
      CBS_len(&wrapper))
    return false;
  ScopedCBB builder;
  CBB new_body, new_wrap, new_extensions;
  if (!CBB_init(builder.get(), tbs.size()) ||
      !CBB_add_asn1(builder.get(), &new_body, CBS_ASN1_SEQUENCE) ||
      !CBB_add_bytes(&new_body, CBS_data(&original), prefix) ||
      !CBB_add_asn1(&new_body, &new_wrap, tag) ||
      !CBB_add_asn1(&new_wrap, &new_extensions, CBS_ASN1_SEQUENCE))
    return false;
  bool found = false;
  while (CBS_len(&extensions)) {
    CBS element, seq, ext_oid;
    if (!CBS_get_asn1_element(&extensions, &element, CBS_ASN1_SEQUENCE))
      return false;
    CBS copy = element;
    if (!CBS_get_asn1(&copy, &seq, CBS_ASN1_SEQUENCE) ||
        !CBS_get_asn1(&seq, &ext_oid, CBS_ASN1_OBJECT))
      return false;
    if (CBS_mem_equal(&ext_oid, oid, sizeof(oid))) {
      if (found)
        return false;
      found = true;
    } else if (!CBB_add_bytes(&new_extensions, CBS_data(&element),
                              CBS_len(&element)))
      return false;
  }
  if (!found || !CBB_flush(builder.get()))
    return false;
  ScopedCBB signed_entry;
  CBB data;
  const auto issuer_hash = Hash(issuer.tbs().spki_tlv);
  if (!CBB_init(signed_entry.get(), 64 + CBB_len(builder.get())) ||
      !CBB_add_u16(signed_entry.get(), 1) ||
      !CBB_add_bytes(signed_entry.get(),
                     reinterpret_cast<const uint8_t *>(issuer_hash.data()),
                     issuer_hash.size()) ||
      !CBB_add_u24_length_prefixed(signed_entry.get(), &data) ||
      !CBB_add_bytes(&data, CBB_data(builder.get()), CBB_len(builder.get())) ||
      !CBB_flush(signed_entry.get()))
    return false;
  entry->assign(reinterpret_cast<const char *>(CBB_data(signed_entry.get())),
                CBB_len(signed_entry.get()));
  return true;
}
inline void VerifyScts(const Policy &policy, std::string_view bytes,
                       std::string_view entry, bool embedded, int64_t now,
                       Evidence *evidence) {
  ErrorScope errors;
  CBS raw, list;
  CBS_init(&raw, reinterpret_cast<const uint8_t *>(bytes.data()), bytes.size());
  if (!CBS_get_u16_length_prefixed(&raw, &list) || CBS_len(&raw))
    return;
  // Validate the entire list framing before accepting any entries.
  CBS check = list;
  while (CBS_len(&check)) {
    CBS item;
    if (!CBS_get_u16_length_prefixed(&check, &item) || !CBS_len(&item))
      return;
  }
  while (CBS_len(&list)) {
    CBS item, id, extensions, signature;
    uint8_t version, hash_alg, sig_alg;
    uint64_t timestamp;
    if (!CBS_get_u16_length_prefixed(&list, &item))
      return;
    if (!CBS_get_u8(&item, &version) || version != 0 ||
        !CBS_get_bytes(&item, &id, 32) || !CBS_get_u64(&item, &timestamp) ||
        !CBS_get_u16_length_prefixed(&item, &extensions) ||
        !CBS_get_u8(&item, &hash_alg) || !CBS_get_u8(&item, &sig_alg) ||
        !CBS_get_u16_length_prefixed(&item, &signature)) {
      ++evidence->invalid_scts;
      continue;
    }
    const auto found = policy.logs.find(Bytes(id));
    if (found == policy.logs.end() || hash_alg != 4 ||
        sig_alg !=
            (EVP_PKEY_id(found->second.key.get()) == EVP_PKEY_RSA ? 1 : 3) ||
        timestamp > static_cast<uint64_t>(now) + 60000) {
      ++evidence->invalid_scts;
      continue;
    }
    ScopedCBB message;
    CBB ext;
    if (!CBB_init(message.get(), entry.size() + 128) ||
        !CBB_add_u8(message.get(), 0) || !CBB_add_u8(message.get(), 0) ||
        !CBB_add_u64(message.get(), timestamp) ||
        !CBB_add_bytes(message.get(),
                       reinterpret_cast<const uint8_t *>(entry.data()),
                       entry.size()) ||
        !CBB_add_u16_length_prefixed(message.get(), &ext) ||
        !CBB_add_bytes(&ext, CBS_data(&extensions), CBS_len(&extensions)) ||
        !CBB_flush(message.get()))
      continue;
    ScopedEVP_MD_CTX ctx;
    if (!EVP_DigestVerifyInit(ctx.get(), nullptr, EVP_sha256(), nullptr,
                              found->second.key.get()) ||
        !EVP_DigestVerifyUpdate(ctx.get(), CBB_data(message.get()),
                                CBB_len(message.get())) ||
        !EVP_DigestVerifyFinal(ctx.get(), CBS_data(&signature),
                               CBS_len(&signature))) {
      ++evidence->invalid_scts;
      continue;
    }
    evidence->scts.push_back({&found->second, Bytes(id),
                              static_cast<int64_t>(timestamp), embedded,
                              Bytes(extensions)});
    ++evidence->valid_scts;
  }
}
inline void Collect(const Policy &policy, const ParsedCertificateList &certs,
                    std::string_view tls, int64_t now, Evidence *evidence) {
  if (certs.empty())
    return;
  if (certs.size() > 1) {
    std::string list, entry;
    if (EmbeddedEntry(*certs[0], *certs[1], &list, &entry))
      VerifyScts(policy, list, entry, true, now, evidence);
  }
  ScopedCBB entry;
  CBB certificate;
  const auto der = certs[0]->der_cert();
  if (CBB_init(entry.get(), der.size() + 5) && CBB_add_u16(entry.get(), 0) &&
      CBB_add_u24_length_prefixed(entry.get(), &certificate) &&
      CBB_add_bytes(&certificate, der.data(), der.size()) &&
      CBB_flush(entry.get()))
    VerifyScts(
        policy, tls,
        std::string_view(reinterpret_cast<const char *>(CBB_data(entry.get())),
                         CBB_len(entry.get())),
        false, now, evidence);
  // Chromium 154 enables kCertificateTransparencyIgnoreOcspScts by default.
}
inline bool StaticExtensions(std::string_view bytes) {
  CBS c;
  CBS_init(&c, reinterpret_cast<const uint8_t *>(bytes.data()), bytes.size());
  bool found = false;
  while (CBS_len(&c)) {
    uint8_t type;
    CBS value;
    if (!CBS_get_u8(&c, &type) || !CBS_get_u16_length_prefixed(&c, &value))
      return false;
    if (type == 0) {
      if (found || CBS_len(&value) < 5)
        return false;
      found = true;
    }
  }
  return found;
}
inline bool Complies(const Policy &policy, const ParsedCertificate &leaf,
                     int64_t now, const Evidence &evidence) {
  int64_t issuance = std::numeric_limits<int64_t>::max();
  for (const auto &sct : evidence.scts)
    if (!sct.log->Disqualified(now))
      issuance = std::min(issuance, sct.timestamp);
  bool live_embedded = false, nonembedded = false, rfc = false;
  std::set<std::string> embedded_logs;
  std::string first_operator;
  bool diverse = false;
  for (const auto &sct : evidence.scts) {
    const bool retired = sct.log->Disqualified(now);
    if ((retired && !sct.embedded) ||
        (sct.log->type == 2 && !StaticExtensions(sct.extensions)))
      continue;
    if (!sct.embedded)
      nonembedded = true;
    else {
      live_embedded |= !retired;
      if (!retired ||
          (issuance < *sct.log->retired && sct.timestamp < *sct.log->retired))
        embedded_logs.insert(sct.id);
    }
    if (!diverse) {
      const auto op = sct.log->Operator(sct.timestamp);
      if (first_operator.empty())
        first_operator = op;
      else
        diverse |= first_operator != op;
    }
    rfc |= sct.log->type == 0 || sct.log->type == 1;
  }
  if (!diverse || (policy.require_rfc && !rfc))
    return false;
  if (nonembedded)
    return true;
  int64_t start, end;
  if (!der::GeneralizedTimeToPosixTime(leaf.tbs().validity_not_before,
                                       &start) ||
      !der::GeneralizedTimeToPosixTime(leaf.tbs().validity_not_after, &end))
    return false;
  return live_embedded &&
         embedded_logs.size() >= (end - start > 180 * 86400 ? 3 : 2);
}
inline void Revocation(const Policy &policy, CertPathBuilderResultPath *path,
                       std::string_view staple, int64_t now,
                       Evidence *evidence) {
  ErrorScope errors;
  evidence->crl = 1;
  std::string issuer;
  for (size_t reverse = 0; reverse < path->certs.size(); ++reverse) {
    const size_t i = path->certs.size() - reverse - 1;
    const auto &cert = *path->certs[i];
    const auto hash = Hash(cert.tbs().spki_tlv);
    evidence->interception |= policy.interception.count(hash) != 0;
    bool revoked = policy.blocked.count(hash);
    const auto subject = policy.subjects.find(Hash(cert.tbs().subject_tlv));
    if (subject != policy.subjects.end() && !subject->second.count(hash))
      revoked = true;
    if (reverse != 0) {
      const auto set = policy.issuers.find(issuer);
      const auto raw = cert.tbs().serial_number;
      if (set != policy.issuers.end() && (raw.empty() || !(raw[0] & 0x80))) {
        size_t zeros = 0;
        while (zeros + 1 < raw.size() && raw[zeros] == 0)
          ++zeros;
        const std::string serial(
            reinterpret_cast<const char *>(raw.data() + zeros),
            raw.size() - zeros);
        revoked |= set->second.count(serial) != 0;
        if (i == 0 && !revoked &&
            (!policy.not_after ||
             static_cast<uint64_t>(now / 1000) <= policy.not_after))
          evidence->crl = 2;
      }
    }
    issuer = hash;
    if (revoked) {
      evidence->crl = 3;
      path->errors.GetErrorsForCert(i)->AddError(
          cert_errors::kCertificateRevoked);
      return;
    }
  }
  evidence->ocsp = 1;
  if (!staple.empty() && path->certs.size() > 1) {
    OCSPVerifyResult::ResponseStatus details;
    const auto status =
        CheckOCSP(staple, path->certs[0].get(), path->certs[1].get(),
                  now / 1000, std::nullopt, &details);
    evidence->ocsp = status == OCSPRevocationStatus::GOOD      ? 2
                     : status == OCSPRevocationStatus::REVOKED ? 3
                                                               : 4;
    if (status == OCSPRevocationStatus::REVOKED)
      path->errors.GetErrorsForCert(0)->AddError(
          cert_errors::kCertificateRevoked);
  }
}
} // namespace transmog_security
