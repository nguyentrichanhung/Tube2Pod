#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ] || [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != x86_64 ]; then
    echo 'Run as root on Linux x86_64.' >&2
    exit 1
fi
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
    echo "Usage: sudo $0 /path/to/release/tube2pod" >&2
    exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if ! id tube2pod >/dev/null 2>&1; then
    useradd --system --home /var/lib/tube2pod --shell /usr/sbin/nologin tube2pod
fi
if [ ! -d /var/lib/tube2pod ]; then
    install -d -o tube2pod -g tube2pod -m 0750 /var/lib/tube2pod
fi
install -d -m 0755 /opt/tube2pod /etc/tube2pod
if [ ! -e /etc/tube2pod/tube2pod.env ]; then
    install -m 0600 "$script_dir/tube2pod.env.example" /etc/tube2pod/tube2pod.env
fi

# Let systemd parse the same environment file and apply the service's home restriction.
systemd-run --quiet --wait --pipe --property=User=tube2pod \
    --property=Group=tube2pod --property=ProtectHome=true \
    --property=EnvironmentFile=/etc/tube2pod/tube2pod.env /bin/sh -c '
        set -e
        "${YTDLP_PATH:-yt-dlp}" --version
        "${FFMPEG_PATH:-ffmpeg}" -version
        "${FFPROBE_PATH:-ffprobe}" -version
    '
install -m 0755 "$1" /opt/tube2pod/tube2pod.new
mv -f /opt/tube2pod/tube2pod.new /opt/tube2pod/tube2pod
install -m 0644 "$script_dir/tube2pod.service" /etc/systemd/system/tube2pod.service
systemctl daemon-reload
systemctl enable tube2pod
systemctl restart tube2pod
printf '%s\n' 'Open http://localhost:8402/connection on the Tube2Pod machine (or the configured BIND_OWNER port).'
