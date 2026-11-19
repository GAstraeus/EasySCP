#!/bin/sh
# Rebuilds assets/AppIcon.icns from the Icon Composer file assets/apple_icon.icon.
# Run it after changing the icon, then run ./bundle.sh.
#
# Needs Apple's Icon Composer for its ictool. bundle.sh only needs the .icns,
# so the app can be built without Icon Composer.
#
# usage: ./make-icon.sh [Dark|Default]
#   An .icns holds a single look, so pick which of the icon's appearances to use.
set -eu
cd "$(dirname "$0")"

rendition=${1:-Dark}

ictool=${ICTOOL:-}
for app in \
    "/Applications/Icon Composer.app" \
    "/Volumes/Icon Composer/Icon Composer.app" \
    "/Applications/Xcode.app/Contents/Applications/Icon Composer.app"
do
    if [ -z "$ictool" ] && [ -x "$app/Contents/Executables/ictool" ]; then
        ictool="$app/Contents/Executables/ictool"
    fi
done
if [ -z "$ictool" ]; then
    echo "Can't find Icon Composer. Set ICTOOL=/path/to/Icon Composer.app/Contents/Executables/ictool" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -r "${work:?}"' EXIT
iconset="$work/AppIcon.iconset"
mkdir "$iconset"

# Every size macOS asks for, as name:pixels.
for entry in \
    16x16:16 16x16@2x:32 32x32:32 32x32@2x:64 128x128:128 \
    128x128@2x:256 256x256:256 256x256@2x:512 512x512:512 512x512@2x:1024
do
    name=${entry%%:*}
    pixels=${entry##*:}
    png="$iconset/icon_$name.png"
    # macOS icons sit inside a margin: the shape covers 824/1024 of the image.
    # An edge-to-edge shape gets shrunk onto a grey plate by macOS 26.
    shape=$(( (pixels * 824 + 512) / 1024 ))
    "$ictool" assets/apple_icon.icon --export-image --output-file "$png" \
        --platform macOS --rendition "$rendition" \
        --width "$shape" --height "$shape" --scale 1 >/dev/null
    if [ ! -s "$png" ]; then
        echo "Icon Composer didn't render the $name image" >&2
        exit 1
    fi
    sips --padToHeightWidth "$pixels" "$pixels" "$png" >/dev/null
done

iconutil --convert icns --output assets/AppIcon.icns "$iconset"
echo "Wrote assets/AppIcon.icns ($rendition look)"
