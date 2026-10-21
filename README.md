# Tube2Pod
The script to convert youtube video script into podcast script
## Tube2Pod

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

Install dependencies outside `/home` because the unit sets
`ProtectHome=true`:

```bash
sudo install -m 0755 "$(command -v yt-dlp)" /usr/local/bin/yt-dlp
# Debian/Ubuntu: sudo apt install ffmpeg
```

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
3. Copy `/feed/<token>.xml`.
4. In Apple Podcasts Connect choose **Add Show → Add by URL**.
5. Paste the feed URL.

There is no public Apple submission API. Rotating a feed token immediately
invalidates the old URL and breaks existing subscriptions.

### Backup

Back up `/var/lib/tube2pod` (SQLite, `audio/`, `transcripts/`, `shows/`) and
`/etc/tube2pod/tube2pod.env`. Keep the service stopped or use SQLite's online
backup while running.

### Retention

Cleanup runs at startup and every 24 hours. Settings are stored in the
database and can be changed in the admin UI:

- **Old audio** (`retention_old_audio_days`, default 7): removes audio for
  finished episodes that have not been updated within the retention window.
  Current audio for an episode is never deleted.
- **Temporary files** (`retention_tmp_hours`, default 24): removes stale
  downloads, transcripts, and partial synthesis output.

Backups only retain what exists at backup time; audio older than the
retention window will already be gone.
