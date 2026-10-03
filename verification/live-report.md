# Live browser verification

- Generated: 2026-10-03T18:16:31.798Z
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
| Wikipedia forced H1 | 2026-10-03T18:16:23.829Z | 153.0.8010.12 | 127.0.0.1:50396 | https://www.wikipedia.org/ | Http2/h2 | Http1/http/1.1 | hyper | 1 | 1:request, 1:response | header + DOM |
| Wikipedia forced H2 | 2026-10-03T18:16:24.742Z | 153.0.8010.12 | 127.0.0.1:50834 | https://www.wikipedia.org/ | Http2/h2 | Http2/h2 | hyper | 1 | 1:request, 1:response | header + DOM |
| Cloudflare forced H3 | 2026-10-03T18:16:27.295Z | 153.0.8010.12 | 127.0.0.1:62506 | https://cloudflare-quic.com/ | Http2/h2 | Http3/h3 | quiche | 1 | 1:request, 1:response | header + DOM |
| Cloudflare Auto Alt-Svc upgrade | 2026-10-03T18:16:30.315Z | 153.0.8010.12 | 127.0.0.1:57839 | https://cloudflare-quic.com/?rustymiddle-upgrade=c7a460b8-fe2a-4623-826c-0e08db989c3b | Http2/h2 | Http3/h3 | quiche | 1 | 12:request, 12:response | header + DOM |
| Cloudflare Auto Alt-Svc upgrade warmup | 2026-10-03T18:16:30.315Z | 153.0.8010.12 | 127.0.0.1:57839 | https://cloudflare-quic.com/?rustymiddle-warmup=e65ab0e1-e36e-40f4-a69b-2b10c451f3ff | Http2/h2 | Http2/h2 | hyper | 1 | 1:request, 1:response | Alt-Svc stripped |
| Stopped proxy blocks DIRECT fallback | 2026-10-03T18:16:30.952Z | 153.0.8010.12 | 127.0.0.1:52449 | https://www.wikipedia.org/ | - | - | - | - | - | DIRECT blocked |
