#!/usr/bin/env bash
# Builds the release variants one after another and copies them to dist/ with timings.
# usage: tools/build-all.sh [variant...]
#   variants: all software femtovg skia femtovg-wgpu  (opt-level 3, profile "release")
#             software-s femtovg-s all-s              (opt-level "s", profile "release-s")
set -u
export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/.."
mkdir -p dist
variants=("$@")
[ ${#variants[@]} -eq 0 ] && variants=(all software femtovg skia)
for v in "${variants[@]}"; do
  base="${v%-s}"
  profile=release
  [ "$base" != "$v" ] && profile=release-s
  case "$base" in
    all) feats="--features r-software,r-femtovg,r-skia" ;;
    software) feats="--no-default-features --features r-software" ;;
    femtovg) feats="--no-default-features --features r-femtovg" ;;
    skia) feats="--no-default-features --features r-skia" ;;
    femtovg-wgpu) feats="--no-default-features --features r-femtovg-wgpu" ;;
    *) echo "unknown variant $v"; continue ;;
  esac
  start=$(date +%s)
  if cargo build --profile "$profile" $feats > "dist/build-$v.log" 2>&1; then
    end=$(date +%s)
    cp "target/$profile/lastdrop-slint.exe" "dist/lastdrop-slint-$v.exe"
    size=$(stat -c %s "dist/lastdrop-slint-$v.exe")
    echo "$v OK ${size} bytes $((end-start)) s (profile $profile)" | tee -a dist/build-times.txt
  else
    end=$(date +%s)
    echo "$v FAILED after $((end-start)) s (see dist/build-$v.log)" | tee -a dist/build-times.txt
  fi
done
