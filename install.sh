#!/bin/sh
# Gemma Search 서버 설치 (Linux x64). 이 한 줄로 끝납니다:
#
#   curl -fsSL https://raw.githubusercontent.com/kimbad1992/gemma-desktop-search/main/install.sh | sh
#
# 최신 릴리스를 ~/gemma-search 에 받아(체크섬 확인) 풀고 `gemma-search install`을 실행합니다.
# install은 준비물(llama.cpp·모델)을 받고, 다른 PC가 닿게 열고, systemd 사용자 서비스로 늘 띄운 뒤 참여 코드를 보여 줍니다.
# 관리자 권한(sudo)은 쓰지 않습니다. 다시 실행하면 새 버전으로 바꾸고 서비스를 다시 띄웁니다.
#
# 옵션은 install에 그대로 넘깁니다:   ... | sh -s -- --yes        (받을 것을 묻지 않기)
#                                     ... | sh -s -- --bind 0.0.0.0:21900
# 환경 변수: GEMMA_SEARCH_DIR(설치 폴더, 기본 ~/gemma-search), GEMMA_SEARCH_VERSION(예: v0.10.2, 기본 최신)
set -eu

REPO=kimbad1992/gemma-desktop-search
DIR=${GEMMA_SEARCH_DIR:-$HOME/gemma-search}

say() { printf '%s\n' "$*"; }
die() { printf '설치를 멈췄습니다: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "Linux에서만 씁니다 (지금: $(uname -s)). Windows는 릴리스의 zip을 받으세요."
case "$(uname -m)" in
  x86_64 | amd64) ;;
  *) die "x86_64(64비트 PC)용만 있습니다 (지금: $(uname -m))" ;;
esac
command -v curl > /dev/null 2>&1 || die "curl이 필요합니다"
command -v tar > /dev/null 2>&1 || die "tar가 필요합니다"
if command -v sha256sum > /dev/null 2>&1; then
  SHA="sha256sum"
elif command -v shasum > /dev/null 2>&1; then
  SHA="shasum -a 256"
else
  die "sha256sum이 필요합니다"
fi

TAG=${GEMMA_SEARCH_VERSION:-}
if [ -z "$TAG" ]; then
  TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
    sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1) || true
  [ -n "$TAG" ] || die "최신 버전을 알아내지 못했습니다. 인터넷 연결을 확인하세요."
fi
NAME="gemma-desktop-search-$TAG-linux-x64.tar.gz"
URL="https://github.com/$REPO/releases/download/$TAG"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

say "Gemma Search $TAG 받는 중 → $DIR"
curl -fSL --progress-bar -o "$TMP/$NAME" "$URL/$NAME" || die "$NAME을 받지 못했습니다"
curl -fsSL -o "$TMP/$NAME.sha256" "$URL/$NAME.sha256" || die "체크섬 파일을 받지 못했습니다"
(cd "$TMP" && $SHA -c "$NAME.sha256" > /dev/null) || die "체크섬이 맞지 않습니다 (받다가 깨졌을 수 있습니다). 다시 실행하세요."
mkdir -p "$TMP/x" "$DIR"
tar -xzf "$TMP/$NAME" -C "$TMP/x"
chmod +x "$TMP/x/gemma-search"
# 이름 바꾸기로 넣는다: 서비스가 지금 쓰고 있는 실행 파일도 바꿀 수 있다
for f in gemma-search LICENSE README.md; do
  if [ -e "$TMP/x/$f" ]; then mv -f "$TMP/x/$f" "$DIR/$f"; fi
done
rm -rf "$TMP"
trap - EXIT INT TERM
say "넣었습니다: $DIR/gemma-search"
say ""

# `curl … | sh`로 실행하면 표준 입력이 이 스크립트라 물어볼 수 없다. 터미널이 있으면 거기서 답을 받는다.
if [ -t 0 ]; then
  exec "$DIR/gemma-search" install "$@"
elif (: < /dev/tty) 2> /dev/null; then
  exec "$DIR/gemma-search" install "$@" < /dev/tty
else
  exec "$DIR/gemma-search" install "$@"
fi
