# Chromium hostname registry data

`effective_tld_names.dat` is the unmodified Public Suffix List snapshot used by
Chromium 154.0.8037.99. Only its ICANN section is used by certificate hostname
verification, matching Chromium's EXCLUDE_PRIVATE_REGISTRIES policy.
`icann-rules.txt` is generated from that section with IDNA A-label conversion.
The original notices and full MPL-2.0 license accompany the data.

To roll the snapshot, run:

```powershell
python scripts/update-chromium-psl.py --chromium-version 154.0.8037.99
python scripts/update-chromium-psl.py --check
python scripts/test-chromium-psl.py
python scripts/dev-env.py --run cargo test -p transmog-tls-native hostname
```

Replace the version with the new exact four-part Chromium release. Review the
source diff, generated ICANN rules and `snapshot.json` receipt before committing.
The script supports `--input PATH` for a previously downloaded source file;
normal updates fetch directly from the Chromium source mirror. `--check` is
offline and does not change files. Updating the PSL does not itself roll the
TLS profile or change the pinned BoringSSL implementation.

Source: https://raw.githubusercontent.com/chromium/chromium/154.0.8037.99/net/base/registry_controlled_domains/effective_tld_names.dat
