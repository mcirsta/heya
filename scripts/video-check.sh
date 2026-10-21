#!/usr/bin/env bash
# Invoked inside ui-offscreen.sh; test executable is required, no account access.
set -euo pipefail
heya_test_binary=$1
heya_app_binary=$2
for heya_program in ffmpeg python3; do
    command -v "$heya_program" >/dev/null || { echo "Missing synthetic test tool: $heya_program (install with pacman)." >&2; exit 1; }
done
[[ -x "$heya_app_binary" ]] || { echo 'Build the optimized application before running the decoder lifecycle check.' >&2; exit 1; }
heya_video_dir=$(mktemp -d /tmp/heya-video-check.XXXXXXXX)
trap 'rm -rf -- "$heya_video_dir"' EXIT
ffmpeg -nostdin -hide_banner -loglevel error \
    -f lavfi -i 'testsrc2=size=1280x720:rate=30' -f lavfi -i 'sine=frequency=440:sample_rate=48000' \
    -t 4 -c:v libx264 -preset ultrafast -pix_fmt yuv420p -c:a aac -movflags +faststart "$heya_video_dir/clip.mp4"
ffmpeg -nostdin -hide_banner -loglevel error -i "$heya_video_dir/clip.mp4" -c:v copy -an "$heya_video_dir/silent.mp4"
ffmpeg -nostdin -hide_banner -loglevel error -f lavfi -i 'testsrc2=size=720x1280:rate=30' \
    -t 4 -c:v libx264 -preset ultrafast -pix_fmt yuv420p -movflags +faststart "$heya_video_dir/portrait.mp4"
ffmpeg -nostdin -hide_banner -loglevel error -display_rotation:v:0 90 -i "$heya_video_dir/clip.mp4" -c copy "$heya_video_dir/rotated.mp4"
for heya_clip in clip silent portrait rotated; do
    echo "Checking synthetic H.264 video: $heya_clip"
    heya_aspect=1.77777778
    if [[ "$heya_clip" == portrait || "$heya_clip" == rotated ]]; then heya_aspect=0.5625; fi
    HEYA_TEST_VIDEO_ASPECT="$heya_aspect" HEYA_TEST_VIDEO_FILE="$heya_video_dir/$heya_clip.mp4" \
        "$heya_test_binary" --ignored --exact video_process::tests::real_playback --nocapture
done
python3 "$(dirname -- "${BASH_SOURCE[0]}")/video-worker-check.py" "$heya_app_binary" "$heya_video_dir/clip.mp4"
