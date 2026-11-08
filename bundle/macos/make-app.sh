#!/bin/sh
# Assembles target/<triple>/release/Whisker.app from the built binary.
#
#   cargo build --release --target aarch64-apple-darwin
#   bundle/macos/make-app.sh aarch64-apple-darwin
#
# Universal binary: build both triples, then pass "universal" (needs lipo).
#
# The icon comes from assets/icon.ico with tools that ship with macOS:
#   perl      copies the largest image out of the .ico (PNG entries as is),
#   sips      scales it to the .iconset sizes,
#   iconutil  packs the .iconset into Whisker.icns.
# No code signing: Gatekeeper asks on first launch (right click > Open).
set -eu

TRIPLE="${1:-aarch64-apple-darwin}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT/bundle/macos"

if [ "$TRIPLE" = "universal" ]; then
    OUT="$ROOT/target/universal/release"
    mkdir -p "$OUT"
    lipo -create -output "$OUT/whisker" \
        "$ROOT/target/aarch64-apple-darwin/release/whisker" \
        "$ROOT/target/x86_64-apple-darwin/release/whisker"
else
    OUT="$ROOT/target/$TRIPLE/release"
fi
BIN="$OUT/whisker"
[ -x "$BIN" ] || { echo "missing $BIN; run cargo build --release --target $TRIPLE" >&2; exit 1; }

APP="$OUT/Whisker.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Whisker"
cp "$HERE/Info.plist" "$APP/Contents/Info.plist"

# Icon: largest .ico entry -> PNG -> iconset -> icns.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
perl -e '
    open(my $f, "<:raw", $ARGV[0]) or die "ico: $!";
    local $/; my $d = <$f>;
    my $n = unpack("v", substr($d, 4, 2));
    my ($best, $size, $off) = (-1, 0, 0);
    for my $i (0 .. $n - 1) {
        my ($w, $h, undef, undef, undef, undef, $len, $at) = unpack("CCCCvvVV", substr($d, 6 + 16 * $i, 16));
        $w ||= 256;
        if ($w > $best) { ($best, $size, $off) = ($w, $len, $at); }
    }
    open(my $o, ">:raw", $ARGV[1]) or die "out: $!";
    print $o substr($d, $off, $size);
' "$ROOT/assets/icon.ico" "$WORK/largest.img"

# A PNG entry is used as is; a BMP entry is converted by sips.
sips -s format png "$WORK/largest.img" --out "$WORK/icon.png" >/dev/null

SET="$WORK/Whisker.iconset"
mkdir -p "$SET"
for s in 16 32 128 256; do
    sips -z "$s" "$s" "$WORK/icon.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z "$d" "$d" "$WORK/icon.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$APP/Contents/Resources/Whisker.icns"

echo "$APP"
