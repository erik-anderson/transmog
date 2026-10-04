# Live browser verification

- Generated: 2026-10-04T06:15:45.112Z
- OS: win32-x64
- Cargo.lock SHA-256: `1135cbcc48f91930717f347d34d6ccee7250e3e49e78c071c24014a3687714ad`
- Test CA SHA-256: `19C210684556612DFF247CEED7151F9A7D4E61345B53A8CCE0F4602ADACD6494`
- Chromium trust: durable current-user test CA; no certificate bypass
- CA trust verified before run: true
- CA trust verified after run: true
- CA lifecycle: durable root retained by explicit user choice; run `pwsh ./scripts/remove-live-test-ca.ps1` for exact-thumbprint teardown
- Chromium QUIC: disabled; service workers: blocked; fresh profile: yes
- Playwright route interception: not used

| Case | Completed | Browser | Proxy | Target | Ingress/ALPN | Egress/ALPN | Adapter | Trust generation | Breakpoint events | Proof |
|---|---|---|---|---|---|---|---|---:|---|---|
| Wikipedia forced H1 | 2026-10-04T06:15:36.879Z | 153.0.8010.12 | 127.0.0.1:64338 | https://www.wikipedia.org/ | Http2/h2 | Http1/http/1.1 | hyper | 1 | 1:request, 1:response | header + DOM |
| Wikipedia forced H2 | 2026-10-04T06:15:37.863Z | 153.0.8010.12 | 127.0.0.1:59169 | https://www.wikipedia.org/ | Http2/h2 | Http2/h2 | hyper | 1 | 1:request, 1:response | header + DOM |
| Cloudflare forced H3 | 2026-10-04T06:15:40.653Z | 153.0.8010.12 | 127.0.0.1:59246 | https://cloudflare-quic.com/ | Http2/h2 | Http3/h3 | quiche | 1 | 1:request, 1:response | header + DOM |
| Cloudflare Auto Alt-Svc upgrade | 2026-10-04T06:15:43.130Z | 153.0.8010.12 | 127.0.0.1:62663 | https://cloudflare-quic.com/?rustymiddle-upgrade=24c17c87-aeda-4fc6-a7ed-cab7886b805b | Http2/h2 | Http3/h3 | quiche | 1 | 11:request, 11:response | header + DOM |
| Cloudflare Auto Alt-Svc upgrade warmup | 2026-10-04T06:15:43.130Z | 153.0.8010.12 | 127.0.0.1:62663 | https://cloudflare-quic.com/?rustymiddle-warmup=66bf0e08-f517-42a3-802f-701bffcbcf50 | Http2/h2 | Http2/h2 | hyper | 1 | 1:request, 1:response | Alt-Svc stripped |
| Stopped proxy blocks DIRECT fallback | 2026-10-04T06:15:44.155Z | 153.0.8010.12 | 127.0.0.1:52955 | https://www.wikipedia.org/ | - | - | - | - | - | DIRECT blocked |
