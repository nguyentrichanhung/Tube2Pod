#!/bin/sh
set -eu
umask 022
if [ "$#" -ne 3 ] || [ ! -f "$2" ]; then
    echo "Usage: $0 <target> <binary> <out-dir>" >&2
    exit 1
fi
mkdir -p "$3"
out=$(CDPATH= cd -- "$3" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
case "$1" in
    x86_64-unknown-linux-gnu)
        chmod 755 "$stage"
        mkdir -p "$stage/debian"
        printf 'Source: tube2pod\nSection: sound\nPriority: optional\nMaintainer: Tube2Pod contributors\nStandards-Version: 4.6.0\n\nPackage: tube2pod\nArchitecture: amd64\nDescription: Tube2Pod\n' > "$stage/debian/control"
        binary=$(CDPATH= cd -- "$(dirname -- "$2")" && pwd)/$(basename -- "$2")
        depends=$(cd "$stage" && dpkg-shlibdeps -O -e "$binary")
        depends=${depends#shlibs:Depends=}
        rm -rf "$stage/debian"
        mkdir -p "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/applications"
        cp "$2" "$stage/usr/bin/tube2pod"
        chmod 755 "$stage/usr/bin/tube2pod"
        cat > "$stage/DEBIAN/control" <<CONTROL
Package: tube2pod
Version: 0.1.0
Architecture: amd64
Maintainer: Tube2Pod contributors
Depends: ${depends}, xdg-utils
Description: Turn videos into podcast episodes
 Mở Tube2Pod từ danh sách ứng dụng để hoàn tất thiết lập.
 Open Tube2Pod from the applications list to finish setup.
CONTROL
        cat > "$stage/usr/share/applications/tube2pod.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Tube2Pod
Exec=/usr/bin/tube2pod --desktop
Terminal=false
Categories=AudioVideo;
DESKTOP
        chmod 755 "$stage/usr" "$stage/usr/bin" "$stage/usr/share" "$stage/usr/share/applications"
        chmod 644 "$stage/usr/share/applications/tube2pod.desktop"
        dpkg-deb --build --root-owner-group "$stage" "$out/tube2pod-$1.deb"
        ;;
    aarch64-apple-darwin|x86_64-apple-darwin)
        app="$stage/payload/Applications/Tube2Pod.app/Contents"
        mkdir -p "$app/MacOS" "$app/Resources"
        cp "$2" "$app/MacOS/tube2pod"
        chmod 755 "$app/MacOS/tube2pod"
        cat > "$app/MacOS/launch" <<'LAUNCH'
#!/bin/sh
nohup "$(dirname "$0")/tube2pod" --desktop >/dev/null 2>&1 &
LAUNCH
        chmod 755 "$app/MacOS/launch"
        cat > "$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.tube2pod.desktop</string>
<key>CFBundleName</key><string>Tube2Pod</string>
<key>CFBundleDisplayName</key><string>Tube2Pod</string>
<key>CFBundleExecutable</key><string>launch</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
</dict></plist>
PLIST
        mkdir "$stage/resources"
        printf '%s\n' '<html><body><h2>Mở Tube2Pod từ Applications để hoàn tất thiết lập.</h2><p>Open Tube2Pod from Applications to finish setup.</p></body></html>' > "$stage/resources/conclusion.html"
        pkgbuild --root "$stage/payload" --identifier org.tube2pod.desktop --version 0.1.0 --install-location / "$stage/component.pkg"
        cat > "$stage/distribution.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<installer-gui-script minSpecVersion="1">
<title>Tube2Pod</title>
<conclusion file="conclusion.html"/>
<options customize="never" require-scripts="false"/>
<choices-outline><line choice="default"/></choices-outline>
<choice id="default" visible="false"><pkg-ref id="org.tube2pod.desktop"/></choice>
<pkg-ref id="org.tube2pod.desktop" version="0.1.0">component.pkg</pkg-ref>
</installer-gui-script>
XML
        mkdir "$stage/packages"
        mv "$stage/component.pkg" "$stage/packages/component.pkg"
        productbuild --distribution "$stage/distribution.xml" --resources "$stage/resources" --package-path "$stage/packages" "$out/tube2pod-$1.pkg"
        rm -rf "$stage/resources" "$stage/packages" "$stage/distribution.xml"
        ;;
    *) echo 'Unsupported target' >&2; exit 1 ;;
esac
