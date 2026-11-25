#!/bin/sh
set -eu
if [ "$#" -ne 3 ] || [ ! -f "$2" ]; then
    echo "Usage: $0 <target> <binary> <out-dir>" >&2
    exit 1
fi
case "$1" in
    x86_64-unknown-linux-gnu|aarch64-apple-darwin|x86_64-apple-darwin) ;;
    *) echo 'Unsupported Unix target' >&2; exit 1 ;;
esac
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$3"
out_dir=$(CDPATH= cd -- "$3" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
name="tube2pod-$1"
mkdir "$stage/$name"
cp "$2" "$stage/$name/tube2pod"
cp "$source_dir/start.sh" "$source_dir/portable.env.example" "$source_dir/Caddyfile.example" "$source_dir/portable.md" "$source_dir/direct-https.md" "$stage/$name/"
chmod 755 "$stage/$name/tube2pod" "$stage/$name/start.sh"
if [ "$1" = x86_64-unknown-linux-gnu ]; then
    mkdir "$stage/$name/deploy"
    cp "$source_dir/install-linux-x86_64.sh" "$source_dir/tube2pod.service" "$source_dir/tube2pod.env.example" "$stage/$name/deploy/"
    chmod 755 "$stage/$name/deploy/install-linux-x86_64.sh"
fi
tar -czf "$out_dir/$name.tar.gz" -C "$stage" "$name"
printf '%s\n' "$out_dir/$name.tar.gz"
