#!/bin/sh
# Build a standalone, ad hoc signed macOS bundle and optional testing DMG.
set -eu
cd "$(dirname "$0")/.."
if [ "$(uname -s)" != Darwin ]; then
    echo "Build this bundle on macOS with Apple's Command Line Tools installed." >&2
    exit 1
fi
heya_target=""
heya_package=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --target) heya_target=${2:?Missing target}; shift 2 ;;
        --package) heya_package=true; shift ;;
        *) echo "Usage: $0 [--target aarch64-apple-darwin|x86_64-apple-darwin] [--package]" >&2; exit 2 ;;
    esac
done
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-15.0}
if [ -n "$heya_target" ]; then
    case "$heya_target" in
        aarch64-apple-darwin) heya_arch=arm64 ;;
        x86_64-apple-darwin) heya_arch=x86_64 ;;
        *) echo "Unsupported macOS target: $heya_target" >&2; exit 2 ;;
    esac
    cargo build --release --locked --target "$heya_target"
    heya_binary="target/$heya_target/release/heya"
    heya_bundle="target/macos-$heya_arch/Heya.app"
else
    cargo build --release --locked
    heya_binary=target/release/heya
    heya_bundle=target/macos/Heya.app
    heya_arch=$(lipo -archs "$heya_binary")
fi
[ "$(lipo -archs "$heya_binary")" = "$heya_arch" ]
heya_version=$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "heya"))')
heya_build=${HEYA_BUILD_NUMBER:-$(git rev-list --count HEAD)}
mkdir -p "$heya_bundle/Contents/MacOS" "$heya_bundle/Contents/Resources"
cp "$heya_binary" "$heya_bundle/Contents/MacOS/heya.new"
mv "$heya_bundle/Contents/MacOS/heya.new" "$heya_bundle/Contents/MacOS/heya"
cp -R third_party/licenses "$heya_bundle/Contents/Resources/"
python3 - "$heya_bundle" "$heya_version" "$heya_build" "$MACOSX_DEPLOYMENT_TARGET" <<'PY'
import pathlib,plistlib,re,sys
bundle,version,build,minimum=sys.argv[1:]
assert re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?',version)
assert re.fullmatch(r'\d+',build)
assert re.fullmatch(r'\d+\.\d+(?:\.\d+)?',minimum)
info={
 'CFBundleIdentifier':'org.heya.Heya','CFBundleName':'Heya','CFBundleDisplayName':'Heya',
 'CFBundleExecutable':'heya','CFBundlePackageType':'APPL',
 'CFBundleShortVersionString':version.split('-')[0],'CFBundleVersion':build,
 'CFBundleGetInfoString':'Heya '+version,
 'NSHighResolutionCapable':True,'NSPrincipalClass':'NSApplication',
 'NSMicrophoneUsageDescription':'Record a voice message when you press the microphone button.',
 'LSMinimumSystemVersion':minimum,
}
with (pathlib.Path(bundle)/'Contents/Info.plist').open('wb') as f:plistlib.dump(info,f)
PY
plutil -lint "$heya_bundle/Contents/Info.plist"
codesign --force --sign - "$heya_bundle"
codesign --verify --deep --strict "$heya_bundle"
python3 - "$heya_bundle/Contents/MacOS/heya" "$MACOSX_DEPLOYMENT_TARGET" <<'PY'
import re,subprocess,sys
binary,minimum=sys.argv[1:]
build=subprocess.check_output(['vtool','-show-build',binary],text=True)
assert re.search(r'\bminos\s+'+re.escape(minimum)+r'\s',build),build
for line in subprocess.check_output(['otool','-L',binary],text=True).splitlines()[1:]:
 library=line.strip().split()[0]
 assert library.startswith(('/System/Library/','/usr/lib/')), 'Non-system runtime dependency: '+library
PY
echo "Built $heya_bundle ($heya_arch, macOS $MACOSX_DEPLOYMENT_TARGET+)"
if [ "$heya_package" = true ]; then
    heya_name="Heya-$heya_version-macos-$heya_arch.dmg"
    mkdir -p target/dist
    heya_stage=$(mktemp -d target/dist/.heya-dmg.XXXXXX)
    trap 'rm -rf -- "$heya_stage"' EXIT HUP INT TERM
    ditto "$heya_bundle" "$heya_stage/Heya.app"
    ln -s /Applications "$heya_stage/Applications"
    hdiutil create -volname "Heya $heya_version" -srcfolder "$heya_stage" \
        -format UDZO -ov "target/dist/$heya_name"
    hdiutil verify "target/dist/$heya_name"
    (cd target/dist && shasum -a 256 "$heya_name" > "$heya_name.sha256")
    echo "Packaged target/dist/$heya_name"
fi
