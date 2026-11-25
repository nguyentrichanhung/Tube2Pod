#!/bin/sh
set -eu
package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
config=${1:-"$package_dir/tube2pod.env"}
case $config in
    /*) ;;
    *) config="$(pwd)/$config" ;;
esac
cd "$package_dir"
exec "$package_dir/tube2pod" --config "$config"
