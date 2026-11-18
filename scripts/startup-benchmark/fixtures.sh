#!/bin/sh
set -eu
dir=${1:?Pass a new fixture directory}
mkdir -p "$dir/hls"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i 'testsrc2=size=640x360:rate=24:duration=90' -f lavfi -i 'sine=frequency=220:duration=90' -c:v libx264 -preset ultrafast -g 48 -c:a aac -movflags +faststart -shortest "$dir/a.mp4"
ffmpeg -hide_banner -loglevel error -y -i "$dir/a.mp4" -c copy "$dir/b.mkv"
cp "$dir/b.mkv" "$dir/c.mkv"
ffmpeg -hide_banner -loglevel error -y -i "$dir/a.mp4" -c copy -hls_time 2 -hls_list_size 0 -hls_segment_filename "$dir/hls/seg_%03d.ts" "$dir/hls/master.m3u8"
printf 'WEBVTT\n\n00:00:00.000 --> 00:00:05.000\nSynthetic subtitle\n' > "$dir/side.vtt"
