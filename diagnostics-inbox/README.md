# ClipClub diagnostic inbox

This small Cloudflare Worker accepts reports from opted-in test computers over HTTPS. It stores report JSON in a **private R2 bucket**, provides a read-key-protected browser inbox, and supports downloading reports into the development workspace. It is independent of ClipClub's peer mesh.

## Deploy

The entry point is `worker.mjs`, a self-contained ES module with no dependencies or build step. Deploy it through the Cloudflare API/connector or the Cloudflare `cf` CLI for this new project; no Wrangler configuration is used.

Create a private R2 bucket and bind it as `REPORTS`. Keep public `r2.dev` and public custom-domain access disabled. Generate two different cryptographically random keys of at least 32 characters; 32 random bytes encoded as 64 hexadecimal characters are suitable. Bind them as Worker **secrets**:

| Binding | Purpose |
| --- | --- |
| `REPORTS` | R2 bucket for reports |
| `UPLOAD_TOKEN` | Upload-only key installed on test computers |
| `READ_TOKEN` | Read-only key used by the browser inbox and pull command |

Use compatibility date `2026-11-10` or later. Enable Workers observability and traces in deployment metadata. The Worker only logs a fixed storage-error event, with no request body, report content, authorization header, or raw provider error. Avoid enabling extra request/header/body logging that would collect credentials or reports.

Apply an R2 lifecycle rule to **delete objects under `reports/` after 30 days**. Lifecycle deletion is asynchronous; it is not an exact 30-day access cutoff. The service does not enable public bucket access. Upload keys cannot list, read, or overwrite reports. Read keys cannot upload. Key rotation updates the corresponding secret; all machines sharing that upload key must then receive its replacement.

The deployment URL is the HTTPS **origin**, without a path, query string, or credentials. In ClipClub → Diagnostics → Private diagnostic inbox, enter this URL, the upload key, a device label, and an optional shared test-session label. Enable automatic uploads. Each completed diagnostic run is then queued durably and retried when the inbox is reachable. Disable automatic uploads to pause sends. Uploads include report observations, peer/network addresses, the checklist, and tester notes; never put passwords or private keys in notes.

Open the Worker URL in a browser and enter the read key to inspect reports. The browser keeps the key only in the tab's memory; reload or close the tab to clear it. The login shell and its static script/style contain no report data. All API reads require authorization.

## Pull reports into this workspace

Configure once from the repository root. The read key is prompted privately and is never an argument:

```sh
python3 scripts/diagnostic-inbox.py --endpoint https://YOUR-INBOX.workers.dev configure
python3 scripts/diagnostic-inbox.py list
python3 scripts/diagnostic-inbox.py pull
python3 scripts/diagnostic-inbox.py pull --session hotspot-a
```

Configuration is saved in ignored `.build/diagnostic-inbox.json` with private file permissions. Reports are downloaded as matching `.json` and `.txt` files into ignored `.build/diagnostics/`. Existing report files are preserved; partial downloads are installed atomically. The CLI rejects unsafe report IDs and symlink destinations, follows pagination even when filtered pages are empty, and refuses redirects so the read key is not forwarded elsewhere.

The reader sends a descriptive `ClipClub/0.1.3 (diagnostic inbox client)` User-Agent. Cloudflare's Browser Integrity Check can reject Python's default `Python-urllib` User-Agent before the request reaches a `workers.dev` Worker (HTTP 403, error 1010). Keep the application User-Agent when adapting this client rather than disabling account protections.

For unattended access, `CLIPCLUB_DIAGNOSTICS_ENDPOINT` and `CLIPCLUB_DIAGNOSTICS_READ_TOKEN` override saved settings. Keep these out of shell history and logs. `--config PATH` and `pull --output PATH` choose alternate locations. Only HTTPS endpoints are accepted, apart from HTTP loopback origins for local tests.

## API contract

All API responses use `Cache-Control: no-store`. Requests use `Authorization: Bearer KEY`. Keys are compared using SHA-256 and Workers' constant-time Web Crypto comparison.

| Request | Key | Result |
| --- | --- | --- |
| `PUT /v1/reports/{id}` | Upload key | Create a report; `201 {"id":"…"}` or `200` for the identical retry |
| `GET /v1/reports?cursor=…&session=…` | Read key | `{"reports":[metadata],"cursor":null}` or a continuation cursor |
| `GET /v1/reports/{id}` | Read key | Full report JSON |

Report IDs are exactly 32 lowercase hexadecimal characters. JSON has exactly these fields:

```json
{
  "schema_version": 1,
  "id": "0123456789abcdef0123456789abcdef",
  "generated_at_ms": 1794312000000,
  "device": "Linux laptop",
  "session": "hotspot-a",
  "suite": "network",
  "app_version": "0.1.3",
  "platform": "linux/x86_64",
  "report": "ClipClub diagnostics\n…"
}
```

`generated_at_ms` is a nonnegative safe JSON integer. `device` allows 1–80 Unicode characters, `session` 0–80, `app_version` 1–40, and `platform` 1–80. Labels cannot contain control characters. The suite is `all`, `device`, `network`, `media`, `wallet`, or `checklist`. Report text is nonempty and at most 240 KiB in UTF-8; the entire JSON request is at most 256 KiB, enforced while reading its stream.

Objects use keys `reports/{id}.json` and include custom metadata for device, session, suite, generation time, app version, and platform. Atomic conditional creates prevent concurrent retry overwrites. Reusing an ID with different content returns `409`. Listings are paginated in object-key order and include `uploaded_at`; filtering can produce empty pages with a continuation cursor, which clients must still follow. No report, filename, wallet file, profile, or log directory is discovered automatically by the service: it accepts only the explicitly submitted report envelope.

## Checks

```sh
node --test diagnostics-inbox/worker.test.mjs
python3 scripts/diagnostic-inbox.test.py
```

The Worker tests use a fake R2 binding and Node's native constant-time comparison to exercise validation, role isolation, conditional creates, concurrent retries, pagination, body limits, safe errors, and viewer rendering. Verify a synthetic upload/list/read against the deployed Worker as a separate integration check. Python tests run an ephemeral loopback HTTP server to exercise credentials, pagination, redirect rejection, private files, and report downloads.

Reference: [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/), [Workers Web Crypto](https://developers.cloudflare.com/workers/runtime-apis/web-crypto/), and [R2 lifecycle rules](https://developers.cloudflare.com/r2/buckets/object-lifecycles/).
