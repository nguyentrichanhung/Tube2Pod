# Tube2Pod

YouTube URL → RSS podcast. Tube2Pod fetches captions, builds an extractive
summary in Rust, sends the spoken script to Omnivoice, assembles MP3s with
ffmpeg, and publishes one private RSS feed per show.

### Local development

Requirements: Rust 1.85+, `yt-dlp`, `ffmpeg`, `ffprobe`, and a reachable
Omnivoice instance.

```bash
export PUBLIC_BASE_URL=https://pod.example.ts.net:8443
export OMNIVOICE_TAILNET_USER=owner@example.com
export DATA_DIR=./data
export YTDLP_PATH=yt-dlp
export FFMPEG_PATH=ffmpeg
export FFPROBE_PATH=ffprobe

cargo test
cargo run
```

Open `http://127.0.0.1:8400` for admin. The public listener is
`http://127.0.0.1:8401`; feed URLs still use `PUBLIC_BASE_URL`.

### Omnivoice authentication

Tube2Pod supports two Omnivoice authentication modes:

- **Tailscale (default):** set `OMNIVOICE_AUTH_MODE=tailscale` and
  `OMNIVOICE_TAILNET_USER=<tailnet identity>`. Tube2Pod sends that identity to
  Omnivoice, which must trust the tailnet connection. This is the recommended
  mode when both services run on the same tailnet.
- **Password:** set `OMNIVOICE_AUTH_MODE=password` and
  `OMNIVOICE_PASSWORD=<long random secret>`. Tube2Pod logs in through
  Omnivoice's `/api/auth/login`, stores the returned session cookie, and
  retries once after a `401`. Use this when Omnivoice is not protected by
  Tailscale identity.

The populated environment file is a secret. Keep
`/etc/tube2pod/tube2pod.env` at mode `0600`, owned by `root:root` or
`tube2pod`. Never commit it or put real credentials in
`deploy/tube2pod.env.example`.

### Production install

```bash
cargo build --release

sudo useradd --system --home /var/lib/tube2pod --shell /usr/sbin/nologin tube2pod
sudo install -d -o tube2pod -g tube2pod -m 0750 /var/lib/tube2pod
sudo install -d -m 0755 /opt/tube2pod /etc/tube2pod

sudo install -m 0755 target/release/tube2pod /opt/tube2pod/tube2pod
sudo install -m 0644 deploy/tube2pod.service /etc/systemd/system/tube2pod.service
sudo install -m 0600 deploy/tube2pod.env.example /etc/tube2pod/tube2pod.env
sudo editor /etc/tube2pod/tube2pod.env
```

The unit sets `ProtectHome=true`, so tools installed under `/home` (including
pip user installs and virtualenvs) are invisible to the service. Install
`yt-dlp`, `ffmpeg`, and `ffprobe` system-wide instead.

Install the distro package and expose it at the configured path, or install
the official standalone `yt-dlp` binary directly:

```bash
# Option A: distro package (Debian/Ubuntu installs /usr/bin/yt-dlp).
sudo apt install yt-dlp ffmpeg
sudo ln -sf /usr/bin/yt-dlp /usr/local/bin/yt-dlp

# Option B: official standalone yt-dlp binary.
sudo curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp \
  -o /usr/local/bin/yt-dlp
sudo chmod 0755 /usr/local/bin/yt-dlp
```

Verify every tool as the service user with an empty environment, using the
absolute paths configured in `/etc/tube2pod/tube2pod.env`:

```bash
sudo -u tube2pod env -i /usr/local/bin/yt-dlp --version
sudo -u tube2pod env -i /usr/bin/ffmpeg -version
sudo -u tube2pod env -i /usr/bin/ffprobe -version
```

If the tools live elsewhere, update `YTDLP_PATH`, `FFMPEG_PATH`, and
`FFPROBE_PATH` to their absolute paths after installation.

Start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now tube2pod
sudo systemctl status tube2pod
curl -fsS http://127.0.0.1:8400/healthz
```

### Tailscale Serve and Funnel

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
   `https://YOUR-MACHINE.YOUR-TAILNET.ts.net:8443/feed/<token>.xml`. Use the
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

Cleanup runs at startup and every 24 hours. Settings are stored in the
database and can be changed in the admin UI. Values are clamped to at least
one day/hour, and the UI accepts 1–365 days and 1–720 hours.

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
