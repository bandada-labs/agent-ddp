#!/bin/sh
# Publishes agent-ddp by hand from the binaries of a GitHub release, when the staged one from the release
# workflow can't be used. npm asks for your 2FA code.
# Usage: npm/publish.sh 0.1.4 [--otp=123456]
set -eu
version="$1"
shift
cd "$(dirname "$0")"
grep -q "\"version\": \"$version\"" package.json || { echo "package.json is not at $version"; exit 1; }
rm -f bin/ddp-*
for platform in darwin-arm64 darwin-x64 linux-arm64 linux-x64 win32-x64.exe; do
  curl -fsSL -o "bin/ddp-$platform" "https://github.com/bandada-labs/agent-ddp/releases/download/v$version/ddp-$platform"
done
chmod +x bin/ddp-*
cp ../README.md ../LICENSE .
./bin/ddp-$(node -p 'process.platform + "-" + process.arch') --version
npm publish --access public "$@"
rm -f bin/ddp-* README.md LICENSE
