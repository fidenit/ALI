#!/bin/sh
# ALI 빌드·실행 스크립트 (Mac)
#   ./run.sh            빌드해서 실행 (개발용. 데이터는 이 폴더의 data, vault 를 쓴다)
#   ./run.sh build      빌드만
#   ./run.sh test       테스트
#   ./run.sh pack       들고 다닐 수 있는 폴더 portable/ALI 를 만들거나 갱신
#
# portable/ALI 폴더는 그 자체로 완결이다. 통째로 어디에 복사해도 그대로 동작한다.
#   ALI.app         프로그램 (설치 불필요)
#   key.txt         Claude API 키
#   jev.txt         (선택) TypeSafe Jev 키
#   eleven.txt      (선택) ElevenLabs 키
#   data/           보드 상태와 설정
#   vault/          기록과 기억 (옵시디언 보관함)
set -e
cd "$(dirname "$0")"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

case "$1" in
  build) cargo build --release ;;
  test)  cargo test ;;
  pack)
    cargo build --release
    out="portable/ALI"
    app="$out/ALI.app"
    first=0
    [ -d "$out" ] || first=1
    pkill -f "$app/Contents/MacOS/ali" 2>/dev/null || true
    mkdir -p "$app/Contents/MacOS"
    cp target/release/ali "$app/Contents/MacOS/ali"
    version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>ALI</string>
  <key>CFBundleDisplayName</key><string>ALI</string>
  <key>CFBundleIdentifier</key><string>com.fidenit.ali</string>
  <key>CFBundleExecutable</key><string>ali</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>마이크 대화에서 말을 받아쓰는 데 씁니다.</string>
</dict>
</plist>
EOF
    # 서명이 아예 없으면 Apple Silicon 에서 실행되지 않고 마이크 권한도 묻지 못한다. 임시 서명(ad-hoc)을 붙인다.
    codesign --force --deep -s - "$app"
    if [ "$first" = 1 ]; then
      # 처음 만들 때만 지금까지의 상태를 옮겨 준다. 그 뒤로는 프로그램만 새것으로 바꾼다.
      for item in key.txt jev.txt eleven.txt voice.txt data vault; do
        [ -e "$item" ] && cp -R "$item" "$out/"
      done
    fi
    echo "만들었습니다: $out"
    ;;
  *) cargo build --release && cargo run --release ;;
esac
