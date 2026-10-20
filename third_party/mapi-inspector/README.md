# Pinned MAPI inspector reference

The 651 source files in `Source` are an unchanged, byte-preserved subset of
[OfficeDev/Office-Inspectors-for-Fiddler](https://github.com/OfficeDev/Office-Inspectors-for-Fiddler)
at `7a76470574377d14cae612c507f45b34d70e05a0`. `source.json` records every SHA-256.
The upstream MIT license is retained in `LICENSE`.

The subset contains all BlockParser and protocol parser files, plus MAPIParser,
SessionNavigator, SessionExtensions and Utilities. Fiddler's windows, controls,
capture hooks, resources, build output and capture/export integration are excluded.
`Parsers/BaseStructure.cs`, the legacy WinForms presentation adapter, is retained
as reference source but excluded from the headless build.

`python scripts/mapi_reference.py --verify` checks the exact inventory and hashes.
`python scripts/mapi_reference.py --inventory` builds the developer-only .NET 10
oracle and lists its concrete parser blocks. Without options it accepts one JSON
test case per stdin line and emits a structured block tree, exact decimal ranges
and scalar values. It has no access to an app capture or active extension worker.
All generated and executable build output belongs under `.tools/mapi-build`.

The oracle retains the original native binary reader and protocol logic. Its
headless adapters remove the fallback to Fiddler's global UI session list and
legacy capture-file JSON export. Its session array is supplied by each fixture;
an absent array stays empty. `extensions/mapi/build/Headless.cs` implements only
the read-only header/session members used by the parser. This is a differential
reference, not a shippable native visualizer. The production extension must emit
ordinary static JavaScript and run through Transmog's existing sandbox.

The bundled visualizer is an independent TypeScript implementation; no parser
behavior or .NET runtime is compiled into the app. Only enum tables are generated
from this source, with the MIT notice retained in the extension, desktop Credits
and supply-chain notices. See [implementation and qualification](../../extensions/mapi/README.md).

No sanitized Office captures are available. Synthetic differential checks cover
broad protocol behavior, bounded resources and derived-source provenance, with
native sandbox and desktop qualification. Real Office end-to-end validation is
deferred and production-capture compatibility is not claimed.
