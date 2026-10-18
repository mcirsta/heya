#!/bin/sh
set -eu
cd "$(dirname "$0")"
xcrun clang -fobjc-arc -Wall -Wextra -Werror \
    -framework AVFoundation repro.m -o opus-prepare-repro
