# Live browser verification

- Generated: 2026-10-03T09:45:15.201Z
- OS: win32-x64
- Cargo.lock SHA-256: `bb286db7a965f42dec48526657d624431acebfd4103435e6febd9852560d7c05`
- Test CA SHA-256: `19C210684556612DFF247CEED7151F9A7D4E61345B53A8CCE0F4602ADACD6494`
- Chromium trust: durable current-user test CA; no certificate bypass
- CA trust verified before run: true
- CA trust verified after run: true
- CA lifecycle: durable root retained by explicit user choice; run `pwsh ./scripts/remove-live-test-ca.ps1` for exact-thumbprint teardown
- Chromium QUIC: disabled; service workers: blocked; fresh profile: yes
- Playwright route interception: not used

| Case | Completed | Browser | Proxy | Target | Ingress/ALPN | Egress/ALPN | Adapter | Trust generation | Breakpoint events | Proof |
|---|---|---|---|---|---|---|---|---:|---|---|
| Wikipedia forced H1 | 2026-10-03T09:45:06.919Z | 153.0.8010.12 | 127.0.0.1:65175 | https://www.wikipedia.org/ | Http2/h2 | Http1/http/1.1 | hyper | 1 | 1:request, 1:response | header + DOM |
| Wikipedia forced H2 | 2026-10-03T09:45:07.901Z | 153.0.8010.12 | 127.0.0.1:60946 | https://www.wikipedia.org/ | Http2/h2 | Http2/h2 | hyper | 1 | 1:request, 1:response | header + DOM |
| Cloudflare forced H3 | 2026-10-03T09:45:10.621Z | 153.0.8010.12 | 127.0.0.1:57429 | https://cloudflare-quic.com/ | Http2/h2 | Http3/h3 | quiche | 1 | 1:request, 1:response | header + DOM |
| Cloudflare Auto Alt-Svc upgrade | 2026-10-03T09:45:13.849Z | 153.0.8010.12 | 127.0.0.1:59566 | https://cloudflare-quic.com/?rustymiddle-upgrade=8b5b68be-cdf2-4ab8-a9b4-624f36d06dc8 | Http2/h2 | Http3/h3 | quiche | 1 | 11:request, 11:response | header + DOM |
| Cloudflare Auto Alt-Svc upgrade warmup | 2026-10-03T09:45:13.849Z | 153.0.8010.12 | 127.0.0.1:59566 | https://cloudflare-quic.com/?rustymiddle-warmup=efd5171f-e3c2-4be2-b129-24296870366a | Http2/h2 | Http2/h2 | hyper | 1 | 1:request, 1:response | Alt-Svc stripped |
| Stopped proxy blocks DIRECT fallback | 2026-10-03T09:45:14.591Z | 153.0.8010.12 | 127.0.0.1:54525 | https://www.wikipedia.org/ | - | - | - | - | - | DIRECT blocked |
