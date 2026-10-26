#!/bin/sh
# Build dot-guest as a universal macOS binary (Intel for OSX-KVM, Apple silicon for real
# Macs) and copy it where Dot Home serves it: <data dir>/guest/dot-guest.
#
#   sh scripts/build-guest.sh                      # writes target/guest/dot-guest
#   sh scripts/build-guest.sh /var/lib/dot-home    # also installs it into a data dir
#   scp target/guest/dot-guest server:/var/lib/dot-home/guest/dot-guest   # or copy it yourself
set -eu
cd "$(dirname "$0")/.."
[ "$(uname)" = Darwin ] || { echo "Build dot-guest on a Mac." >&2; exit 1; }
rustup target add x86_64-apple-darwin aarch64-apple-darwin >/dev/null
target="${CARGO_TARGET_DIR:-target}"
cargo build --release --locked -p dot-guest --target x86_64-apple-darwin
cargo build --release --locked -p dot-guest --target aarch64-apple-darwin
mkdir -p target/guest
lipo -create -output target/guest/dot-guest \
  "$target/x86_64-apple-darwin/release/dot-guest" \
  "$target/aarch64-apple-darwin/release/dot-guest"
codesign --force --sign - --identifier dev.dot.guest target/guest/dot-guest
echo "Built target/guest/dot-guest ($(shasum -a 256 target/guest/dot-guest | cut -c1-16)...)"
if [ $# -ge 1 ]; then
  mkdir -p "$1/guest"
  cp target/guest/dot-guest "$1/guest/dot-guest"
  echo "Installed into $1/guest/dot-guest"
fi
