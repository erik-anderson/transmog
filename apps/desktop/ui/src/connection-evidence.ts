import type { SessionDetail } from "./models.js";

type Transport = NonNullable<SessionDetail["performance"]>["transports"][number];

// CONNECT destinations also belong to intercepted TLS and parsed plaintext legs.
export function isOpaqueConnectTransport(transport: Transport): boolean {
  return (
    transport.leg === "upstream" &&
    Boolean(transport.tunnelDestination) &&
    transport.tlsVersion == null &&
    transport.tls == null
  );
}

export function opaqueConnectTransport(detail: SessionDetail): Transport | undefined {
  const transports = detail.performance?.transports ?? [];
  if (
    !detail.requests.some((head) => head.method === "CONNECT") ||
    transports.some(
      (transport) =>
        transport.leg === "client" && (transport.tlsVersion != null || transport.tls != null),
    )
  ) {
    return undefined;
  }
  return transports.find(isOpaqueConnectTransport);
}
