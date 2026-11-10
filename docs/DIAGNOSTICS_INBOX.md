# Private diagnostic inbox

ClipClub can send completed diagnostic reports to a private HTTPS inbox so test
computers do not need to exchange files. Sharing starts disabled in every profile.
The report service is independent of peer discovery and video transfers.

## Connect a test computer

1. Open **Diagnostics → Private diagnostic inbox** in an updated build.
2. Enter the inbox URL and its **upload key**. This key permits sending reports;
   it cannot list or read them.
3. Choose a device nickname and a test-session code. Use the same session code on
   the computers involved in a test, such as `hotspot-a-to-b`.
4. Enable automatic uploads and save the settings.

Completed diagnostic runs now queue automatically, including the webview
observations, checklist, and notes visible in the exported report. After changing
notes or checklist results, choose **Send report to inbox** to send an updated
snapshot. Copy and save remain available.

Sharing includes peer IDs, network addresses, and the notes you enter. Automated
checks omit wallet secrets and provider credentials. Do not put passwords, keys,
or access tokens in notes. The app never attaches profile files, wallet backups,
video files, or arbitrary log files.

## Offline reports

Queued reports are saved inside `diagnostic-inbox/queue/` in the selected profile.
ClipClub retries in the background every 30 seconds while open, including after a
restart. **Retry uploads** requests an immediate retry. A failed upload leaves the
report queued; retrying an acknowledged report does not overwrite or duplicate it
in the inbox.

Turning sharing off pauses queued uploads. Changing the inbox address does not
send old queued reports to a different service; restore the original address to
deliver those reports. The queue holds up to 100 reports, each with at most
240 KiB of report text and 256 KiB of encoded upload data. A full queue shows an
error and leaves the current report available to copy or save.

Settings, the upload key, and queued reports are stored in private local files.
The upload key is never returned by the local API or saved in browser storage.
Blank key input keeps the saved key for the same inbox address. Changing the
address requires a new key. The last-sent indicator describes the current
app run; pending reports and settings survive restarting.

## Read reports on the development computer

The reader uses a separate **read key**. Keep this credential on the development
computer. Reader configuration defaults to `.build/diagnostic-inbox.json`, which
is ignored by Git and must have owner-only permissions.

Configure once if a private reader configuration has not already been prepared:

```sh
python3 scripts/diagnostic-inbox.py configure
```

List reports, then download new JSON and text reports into `.build/diagnostics/`:

```sh
python3 scripts/diagnostic-inbox.py list
python3 scripts/diagnostic-inbox.py pull
python3 scripts/diagnostic-inbox.py pull --session hotspot-a-to-b
```

The inbox URL also serves a report viewer. Enter the read key to list reports,
filter by session, and open the full text. The viewer keeps the key only in tab
memory; locking or reloading clears access.

## Hosting and verification

The service in [`diagnostics-inbox/`](../diagnostics-inbox/) uses one Cloudflare
Worker and a private R2 bucket. Only the Worker has bucket access; no R2 credentials
are included in desktop builds. Upload and reader keys are Worker secrets. The
deployment should expire stored reports after 30 days using the bucket's lifecycle
policy; deletion is asynchronous. Downloaded local copies have no automatic expiry.

The deployed inbox for this workspace is
`https://clipclub-diagnostics.my-account-325.workers.dev`. This address is public;
report access requires the appropriate key. Local setup pages and credentials are
kept under `.build/` and must not be committed.

`scripts/check.sh` includes the report client, interface, Worker, and reader tests.
To exercise a real desktop process against a deployed inbox using the private
`.build/diagnostic-inbox-secrets.json` configuration, run:

```sh
CLIPCROP_FFPROBE="$PWD/.build/native-release-ffprobe/ffprobe" \
  python3 scripts/diagnostic-inbox-integration.py
```

This sends one synthetic report, checks authenticated retrieval, and verifies that
the upload key cannot read it. The synthetic report follows normal server expiry.
