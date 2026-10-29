#!/bin/sh
# Builds AgentDDP with plain swiftc, for an app built without an Xcode project or SwiftPM: a static library and its
# module. Debug only, like the package: the sources are inside `#if DEBUG`, so this passes -D DEBUG.
#
#   ios/build-swiftc.sh [simulator|device] [out dir]      default: simulator, build/agentddp-swiftc
#
# Then build the app with the same SDK and target, and the same -D DEBUG:
#   swiftc ... -D DEBUG -I <out> -L <out> -lAgentDDP
# The system libraries it needs (UIKit, SQLite and the rest) are linked by the module itself.
set -eu

kind=${1:-simulator}
out=${2:-build/agentddp-swiftc}
case "$kind" in
    simulator) sdk=iphonesimulator; target="$(uname -m)-apple-ios15.0-simulator" ;;
    device) sdk=iphoneos; target=arm64-apple-ios15.0 ;;
    *) echo "usage: $0 [simulator|device] [out dir]" >&2; exit 2 ;;
esac

sources="$(dirname "$0")/AgentDDP/Sources/AgentDDP"
mkdir -p "$out"
xcrun --sdk "$sdk" swiftc -parse-as-library -emit-library -static -emit-module -module-name AgentDDP \
    -target "$target" -sdk "$(xcrun --sdk "$sdk" --show-sdk-path)" -D DEBUG -Onone -g \
    -emit-module-path "$out/AgentDDP.swiftmodule" -o "$out/libAgentDDP.a" "$sources"/*.swift
echo "$out/libAgentDDP.a and $out/AgentDDP.swiftmodule for $target"
