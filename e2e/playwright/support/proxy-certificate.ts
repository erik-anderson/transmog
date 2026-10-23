import { createHash, X509Certificate } from "node:crypto";
import { readFileSync } from "node:fs";
import http from "node:http";
import { isIP } from "node:net";
import tls from "node:tls";

/** Authorize the validated fixture leaf and this run's root for alternate TLS key types. */
export function proxyCertificateSpki(
  proxyAddress: string,
  originUrl: string,
  caCertificate: string,
): Promise<string> {
  const origin = new URL(originUrl);
  const hostname = origin.hostname.replace(/^\[|\]$/g, "");
  const proxy = new URL(`http://${proxyAddress}`);
  const ca = readFileSync(caCertificate);
  return new Promise((resolve, reject) => {
    let encrypted: tls.TLSSocket | undefined;
    const request = http.request({
      hostname: proxy.hostname,
      port: proxy.port,
      method: "CONNECT",
      path: origin.host,
    });
    const fail = (error: Error) => {
      clearTimeout(timer);
      encrypted?.destroy();
      request.destroy();
      reject(error);
    };
    const timer = setTimeout(() => fail(new Error("Proxy certificate probe timed out")), 5000);
    request.on("error", fail);
    request.on("connect", (response, socket, head) => {
      if (response.statusCode !== 200 || head.length) {
        socket.destroy();
        fail(new Error("Proxy did not establish the certificate probe tunnel"));
        return;
      }
      try {
        encrypted = tls.connect({
          socket,
          host: hostname,
          servername: isIP(hostname) ? undefined : hostname,
          ca,
          rejectUnauthorized: true,
          ALPNProtocols: ["h2", "http/1.1"],
        });
      } catch (error) {
        socket.destroy();
        fail(error as Error);
        return;
      }
      encrypted.on("error", fail);
      encrypted.once("secureConnect", () => {
        try {
          const certificate = encrypted!.getPeerCertificate().raw;
          if (!certificate) {
            throw new Error("Proxy leaf certificate is missing");
          }
          const publicKey = new X509Certificate(certificate).publicKey.export({
            type: "spki",
            format: "der",
          });
          clearTimeout(timer);
          const rootKey = new X509Certificate(ca).publicKey.export({ type: "spki", format: "der" });
          resolve(
            [publicKey, rootKey]
              .map((key) => createHash("sha256").update(key).digest("base64"))
              .join(","),
          );
          encrypted!.destroy();
        } catch (error) {
          fail(error as Error);
        }
      });
    });
    request.end();
  });
}
