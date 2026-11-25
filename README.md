# Tube2Pod

YouTube URL → RSS podcast. Tube2Pod downloads the selected source audio with
`yt-dlp`, converts the full track to MP3 with `ffmpeg`, measures it with
`ffprobe`, and publishes one RSS feed per show.

### Home publishing without Tailscale

Use the [portable server guide](deploy/portable.md) for Windows x64, Linux x64,
macOS ARM64 or Intel. The package contains the server, launcher and examples;
install media tools separately. Open the owner UI at `http://localhost:8402`.
For Internet listening, follow [direct HTTPS/NAT](deploy/direct-https.md): free
DuckDNS hostname → home router → Caddy → public listener. Administration stays
local. No relay or Omnivoice is required.

Native packages are built by `.github/workflows/server-packages.yml`. Build and
runtime evidence must be checked separately; a configured CI matrix is not a
successful release. Locally verified: Ubuntu 22.04.5 x64 package, real YouTube
audio conversion/publishing, restart persistence and Caddy with a local CA.
Windows/macOS native runs and real Internet NAT remain unverified. Current verification is recorded in the approved implementation
plan under `docs/superpowers/plans/2026-11-25-direct-home-publishing.md`.

### Local development

Requirements: Rust 1.85+, `yt-dlp`, `ffmpeg`, and `ffprobe`.

```bash
export PUBLIC_BASE_URL=https://your-name.duckdns.org
export DATA_DIR=./data
export YTDLP_PATH=yt-dlp
export FFMPEG_PATH=ffmpeg
export FFPROBE_PATH=ffprobe

cargo test
cargo run
```

The admin and public listeners bind to `http://127.0.0.1:8400` and
`http://127.0.0.1:8401`. Manage the server locally at `http://localhost:8402`.
Feed URLs use `PUBLIC_BASE_URL`; publish only the public listener through Caddy.

### Upgrade from the old pipeline

Before upgrading, back up the database and data directory and verify the upgrade
on a copy. On the first upgrade, episodes in `queued`, `running`, or `review`
from the old pipeline are deleted with their files, including any previously
published audio being reread. Other episodes retain their audio, transcripts,
summaries, and URLs. Restarting an interrupted upgrade can resume cleanup.
Do not upgrade a running service until ready to apply this change.

### Install on Linux x86-64

Build the server on Linux x86-64 with `cargo build --release --locked`, or use
an equivalent release binary. Install `yt-dlp`, `ffmpeg` and `ffprobe`
system-wide. On Debian/Ubuntu, for example:

```bash
sudo apt install yt-dlp ffmpeg
# The example environment expects yt-dlp at /usr/local/bin/yt-dlp.
sudo ln -s /usr/bin/yt-dlp /usr/local/bin/yt-dlp
sudo deploy/install-linux-x86_64.sh target/release/tube2pod
```

Run the installer from a checkout or release directory containing the `deploy`
files. It creates the service account and starts the existing systemd unit. It
checks the configured tools as the service user before installing the binary.
Tools under `/home` are inaccessible to the unit because `ProtectHome=true`.
If tools are elsewhere, edit their absolute paths in
`/etc/tube2pod/tube2pod.env` and rerun the installer.

On an existing installation, back up `/var/lib/tube2pod` and the environment
file first, and test the upgrade on a copy as described above. The installer
keeps the existing environment and data directory; it replaces the binary and
unit and restarts the service. In particular it retains an existing
`PUBLIC_BASE_URL`, feed tokens and server identity. The old-pipeline cleanup
described above still applies when the upgraded server starts.

On the server itself, open `http://localhost:8402/connection`, choose **Tạo mã ghép cặp**,
and scan the one-use QR in the Tube2Pod app on the same Wi-Fi. The QR pins the
server identity for the local connection. Create a new QR for each device.
The LAN listener defaults to port 8403; allow local access to it if the host
firewall blocks it. The app can listen locally without a public domain. To
revoke a device, use the device list on the same owner page.

`PUBLIC_BASE_URL` is optional for local use. Keep an existing HTTPS origin when
upgrading; public RSS still requires a working public HTTPS origin or an
available Tube2Pod relay. The owner page can enable remote access when a relay
endpoint is configured; see [relay deployment](deploy/relay.md). Changing an
existing public feed URL is the owner's decision. If the old endpoint remains
under your control, arrange a 301 redirect and verify it before switching.

```bash
sudo systemctl status tube2pod
curl -fsS http://127.0.0.1:8400/healthz
```

### Demo and review preparation

Use only media you have permission to distribute. On an isolated demo server,
create a show, submit an authorized YouTube URL on the website, select its
audio and wait for the full MP3 to become ready. Enable RSS and publish the
episode, then verify the feed and audio from the intended network. On the
server's owner page generate a fresh QR for each reviewer device. In the app,
listen to the published episode, edit the show and publish another ready
episode. Keep the demo server and its access available for the review period;
a used or expired QR needs a new one. If the reviewer cannot reach the demo
server over LAN, agree on another review approach before submitting.

### Optional existing Tailscale deployment

Admin stays tailnet-only on 443; public feed uses Funnel on 8443:

```bash
sudo tailscale serve --bg --https=443 http://127.0.0.1:8400
sudo tailscale funnel --bg --https=8443 http://127.0.0.1:8401
```

`PUBLIC_BASE_URL` must match the Funnel origin exactly, including `:8443`
and without a trailing slash. Do not expose `:8400` with Funnel.

### Feed and Apple Podcasts

1. Create a show in `/shows`.
2. Complete the feed checklist and enable the feed.
3. Open the show's **Feed** tab and copy the absolute Feed URL, for example
   `https://your-name.duckdns.org/feed/<token>.xml`. Use the
   full URL, not just the `/feed/<token>.xml` path.
4. In Apple Podcasts Connect choose **Add Show → Add by URL**.
5. Paste the absolute feed URL.

There is no public Apple submission API. Rotating a feed token immediately
invalidates the old URL and breaks existing subscriptions.

### Backup

Back up `/var/lib/tube2pod` (SQLite, `audio/`, `transcripts/`, `shows/`) and
`/etc/tube2pod/tube2pod.env`. Keep the service stopped or use SQLite's online
backup while running.

### Retention

Cleanup runs at startup and every 24 hours. It reads existing settings from
the database and falls back to the defaults below. Values are clamped to at
least one day/hour.

- **Old or superseded audio** (`retention_old_audio_days`, default 7):
  deletes files in `audio/` that are not the episode's current
  `audio_path`. For `<uuid>-<version>.mp3` files linked to an episode, expiry
  is measured from that episode's `updated_at`. Orphan files (unrecognized
  names or no matching episode) use the file's mtime instead. The current
  audio for every episode is never deleted.
- **Temporary episode directories** (`retention_tmp_hours`, default 24):
  deletes `tmp/<uuid>/` directories for episodes in `failed`, `cancelled`,
  `ready`, or `published` state once the directory's mtime is older than the
  window. `queued` and `running` episodes are skipped, as are `tmp/yt-dlp`
  and non-UUID directories. Orphan UUID directories are removed after the
  same window.
- **Canonical transcripts** in `transcripts/` are **not** removed by
  retention. Back them up; delete them manually if you no longer need them.

Backups only retain what exists at backup time; audio older than the
retention window will already be gone.
