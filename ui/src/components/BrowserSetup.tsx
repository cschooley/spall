// Set up a browser for testing through taphttp (#32), with no terminal or
// Docker knowledge: download the CA certificate (public; the key never
// leaves taphttp), follow the steps for your browser, then check that an
// https request arrives. Everything shown is text.

import { useEffect, useState } from "react";
import { apiJson } from "../lib/engagement.ts";
import { asCaInfo, type CaInfo } from "../lib/ca.ts";
import type { TrafficEntry } from "../types.ts";

const CHECK_HINT_AFTER_MS = 30_000;

export function BrowserSetup({ proxy, latestHttps }: { proxy: string | null; latestHttps: TrafficEntry | null }) {
  const [ca, setCa] = useState<CaInfo | string | null>(null);
  const [checking, setChecking] = useState<{ since: string } | null>(null);
  const [slow, setSlow] = useState(false);

  useEffect(() => {
    let live = true;
    void apiJson("/api/ca").then((body) => {
      if (live) setCa(typeof body === "string" ? body : (asCaInfo(body) ?? "unexpected answer from spall"));
    });
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    if (!checking) return;
    const id = setTimeout(() => setSlow(true), CHECK_HINT_AFTER_MS);
    return () => clearTimeout(id);
  }, [checking]);

  const seen = checking && latestHttps && latestHttps.ts >= checking.since ? latestHttps : null;
  const [host, port] = splitProxy(proxy ?? "127.0.0.1:8080");
  const info = typeof ca === "string" || ca === null ? null : ca;

  return (
    <div className="setup">
      <h4>1. Get the certificate</h4>
      {ca === null ? <p className="empty">loading…</p> : null}
      {typeof ca === "string" ? (
        <p className="error" role="alert">
          {ca}
        </p>
      ) : null}
      {info ? (
        <>
          <dl>
            <dt>SHA-256</dt>
            <dd className="fp">{info.sha256 ?? "unknown"}</dd>
            <dt>subject</dt>
            <dd>{info.subject ?? "unknown"}</dd>
            <dt>valid until</dt>
            <dd>{info.notAfter ?? "unknown"}</dd>
          </dl>
          <p className="actions">
            <a className="button" href="/api/ca.pem" download="taphttp-ca.pem">
              download taphttp-ca.pem
            </a>
            <a className="button" href="/api/ca.der" download="taphttp-ca.der">
              .der
            </a>
          </p>
          <p className="note wrap">
            A phone or VM already set to use the proxy can get it at http://taphttp/ instead. After importing, compare the SHA-256 above with what the browser shows.
          </p>
        </>
      ) : null}

      <h4>2. Set up a browser</h4>
      <details open>
        <summary>Firefox, in its own profile (recommended)</summary>
        <ol>
          <li>Create a profile just for testing: open about:profiles, "Create a New Profile", then "Launch profile in new browser".</li>
          <li>{`Settings → Network Settings → Manual proxy configuration: HTTP proxy ${host}, port ${port}, and tick "Also use this proxy for HTTPS".`}</li>
          <li>In about:config, set network.captive-portal-service.enabled to false (otherwise Firefox's own checks fill your traffic).</li>
          <li>Settings → Privacy & Security → Certificates → View Certificates → Authorities → Import taphttp-ca.pem, tick "Trust this CA to identify websites", and check the fingerprint.</li>
        </ol>
      </details>
      <details>
        <summary>Chromium or Chrome on Linux</summary>
        <p className="note wrap">
          Every Chromium profile on Linux shares one certificate store (~/.pki/nssdb), so trusting the CA there trusts it everywhere Chromium runs. Prefer the
          Firefox profile above.
        </p>
      </details>
      <p className="warn-box">
        Don't trust this CA system-wide or in the browser you use every day: whoever has taphttp's key could then impersonate any site to you.
      </p>

      <h4>3. Check it worked</h4>
      {checking ? (
        seen ? (
          <p className="ok" role="status">{`✓ spall saw ${seen.method} ${seen.url} (${seen.status ?? "no response"}). The browser is set up.`}</p>
        ) : (
          <>
            <p role="status">Waiting for an https request through the proxy… visit any https site in the test browser.</p>
            {slow ? (
              <ul className="hints" aria-label="Nothing yet?">
                <li>{`Nothing arrived: check the browser's proxy is ${host}:${port}, for HTTPS too.`}</li>
                <li>A certificate warning in the browser means the CA isn't trusted yet (step 1 and the import).</li>
                <li>Out-of-scope sites are still captured: scope only limits what spall sends.</li>
              </ul>
            ) : null}
          </>
        )
      ) : (
        <button type="button" onClick={() => (setSlow(false), setChecking({ since: new Date().toISOString() }))}>
          start the check
        </button>
      )}
    </div>
  );
}

function splitProxy(p: string): [string, string] {
  const i = p.lastIndexOf(":");
  return i > 0 ? [p.slice(0, i), p.slice(i + 1)] : [p, "8080"];
}
