#!/usr/bin/env bash
# gjc-connect 반입용 묶음 생성 스크립트 (인터넷이 되는 macOS 빌드 PC에서 실행)
#
# 하는 일:
#   1) 고정한 버전의 gjc 바이너리를 GitHub 릴리스에서 받아 SHA256을 검증
#   2) 대상별로 gjc-connect를 `cargo build --release --target ...`로 빌드
#   3) dist/gjc-connect-<대상>-<버전>.zip|tar.gz 와 dist/SHA256SUMS 생성
#
# 필요한 것: bash, curl, shasum 또는 sha256sum, cargo, zip(Windows 묶음), tar
#
# 대상별 빌드 요건
#   macos-arm64 : rustup target add aarch64-apple-darwin  (macOS 호스트)
#   macos-x64   : rustup target add x86_64-apple-darwin   (macOS 호스트)
#   windows-x64 : 아래 중 하나가 필요하며, 없으면 이 대상은 "건너뜀"으로 표시됩니다.
#                 a) cargo install cargo-xwin  + rustup target add x86_64-pc-windows-msvc
#                 b) cargo install cargo-zigbuild + zig + rustup target add x86_64-pc-windows-gnu
#                 c) mingw-w64(x86_64-w64-mingw32-gcc) + rustup target add x86_64-pc-windows-gnu
#                 (rustup이 없으면 해당 타깃의 표준 라이브러리를 직접 설치해야 합니다)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GJC_VERSION="0.19.3"
TARGETS="windows-x64,macos-arm64,macos-x64"
OUT_DIR="$ROOT/dist"
PORTABLE_GIT=""
STRICT=0
CLEAN=0
REPO="Yeachan-Heo/gajae-code"

usage() {
  cat <<EOF
사용법: $(basename "$0") [옵션]

옵션:
  --version <버전>       묶을 gjc 버전 (기본: $GJC_VERSION)
  --targets <목록>       쉼표로 구분: windows-x64,macos-arm64,macos-x64 (기본: 전체)
  --portable-git <경로>  Windows 묶음에 넣을 PortableGit
                         (압축을 푼 폴더이거나, 받은 PortableGit-*.7z.exe 파일)
  --out <폴더>           결과물 폴더 (기본: $ROOT/dist)
  --strict               빌드를 건너뛴 대상이 하나라도 있으면 실패로 끝냄
  --clean                결과물 폴더를 먼저 비움
  -h, --help             이 도움말
EOF
}

die() { echo "오류: $*" >&2; exit 1; }
info() { echo "==> $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version 값이 없습니다"; GJC_VERSION="$2"; shift 2 ;;
    --targets) [ $# -ge 2 ] || die "--targets 값이 없습니다"; TARGETS="$2"; shift 2 ;;
    --portable-git) [ $# -ge 2 ] || die "--portable-git 값이 없습니다"; PORTABLE_GIT="$2"; shift 2 ;;
    --out) [ $# -ge 2 ] || die "--out 값이 없습니다"; OUT_DIR="$2"; shift 2 ;;
    --strict) STRICT=1; shift ;;
    --clean) CLEAN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "알 수 없는 옵션: $1" ;;
  esac
done

case "$GJC_VERSION" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) die "버전 형식이 올바르지 않습니다: $GJC_VERSION (예: 0.19.3)" ;;
esac

for cmd in curl cargo tar; do
  command -v "$cmd" >/dev/null 2>&1 || die "'$cmd' 명령을 찾을 수 없습니다"
done

if command -v sha256sum >/dev/null 2>&1; then
  sha256_of() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
  sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  die "sha256sum 또는 shasum이 필요합니다"
fi

HOST_OS="$(uname -s)"
HOST_ARCH="$(uname -m)"
CACHE_DIR="$ROOT/.cache/gjc-v$GJC_VERSION"
RELEASE_URL="https://github.com/$REPO/releases/download/v$GJC_VERSION"
SYSROOT="$(rustc --print sysroot 2>/dev/null || true)"

mkdir -p "$CACHE_DIR" "$OUT_DIR"
if [ "$CLEAN" = 1 ]; then
  rm -rf "${OUT_DIR:?}"/*
fi

# ---------------------------------------------------------------------------
# gjc 바이너리 다운로드 + 체크섬 검증
# ---------------------------------------------------------------------------
CHECKSUM_FILE="$CACHE_DIR/gajae-release-binaries.sha256"
if [ ! -s "$CHECKSUM_FILE" ]; then
  info "체크섬 파일 내려받기: gajae-release-binaries.sha256"
  curl -fsSL --retry 3 --connect-timeout 15 -o "$CHECKSUM_FILE.part" \
    "$RELEASE_URL/gajae-release-binaries.sha256" \
    || die "체크섬 파일을 받지 못했습니다 ($RELEASE_URL). 버전과 인터넷 연결을 확인하세요"
  mv "$CHECKSUM_FILE.part" "$CHECKSUM_FILE"
fi

# fetch_gjc <자산 이름> : 검증된 파일 경로를 CACHE_DIR 아래에 준비한다 (실패 시 종료)
fetch_gjc() {
  local asset="$1"
  local file="$CACHE_DIR/$asset"
  local expected actual
  expected="$(awk -v a="$asset" '$2 == a {print $1}' "$CHECKSUM_FILE")"
  [ -n "$expected" ] || die "체크섬 파일에 $asset 항목이 없습니다"
  if [ -f "$file" ] && [ "$(sha256_of "$file")" = "$expected" ]; then
    info "캐시 사용: $asset (SHA256 일치)"
    return 0
  fi
  info "내려받기: $asset"
  curl -fsSL --retry 3 --connect-timeout 15 -o "$file.part" "$RELEASE_URL/$asset" \
    || die "$asset 을(를) 받지 못했습니다"
  actual="$(sha256_of "$file.part")"
  if [ "$actual" != "$expected" ]; then
    rm -f "$file.part"
    die "$asset SHA256 불일치 (기대값 $expected, 실제값 $actual). 받은 파일을 폐기했습니다"
  fi
  mv "$file.part" "$file"
  info "SHA256 검증 통과: $asset"
}

# ---------------------------------------------------------------------------
# 대상별 빌드
# ---------------------------------------------------------------------------
have_rust_target() { [ -n "$SYSROOT" ] && [ -d "$SYSROOT/lib/rustlib/$1" ]; }

# build_gjc_connect <rust target> <os: windows|macos> ; 성공 시 바이너리 경로를 BUILT_BIN에 둔다.
# 빌드할 수 없으면 SKIP_REASON을 설정하고 1을 돌려준다.
build_gjc_connect() {
  local rt="$1" os="$2" ext=""
  BUILT_BIN=""
  SKIP_REASON=""
  [ "$os" = windows ] && ext=".exe"

  if [ "$os" = macos ]; then
    if [ "$HOST_OS" != "Darwin" ]; then
      SKIP_REASON="macOS 묶음은 macOS 빌드 PC에서만 만들 수 있습니다 (현재 호스트: $HOST_OS)"
      return 1
    fi
    if ! have_rust_target "$rt"; then
      SKIP_REASON="Rust 타깃 $rt 이(가) 설치되어 있지 않습니다 (rustup target add $rt)"
      return 1
    fi
    info "빌드: cargo build --release --locked --target $rt"
    (cd "$ROOT" && cargo build --release --locked --target "$rt") || die "$rt 빌드 실패"
  else
    if command -v cargo-xwin >/dev/null 2>&1 && have_rust_target x86_64-pc-windows-msvc; then
      rt="x86_64-pc-windows-msvc"
      info "빌드: cargo xwin build --release --locked --target $rt"
      (cd "$ROOT" && cargo xwin build --release --locked --target "$rt") || die "$rt 빌드 실패"
    elif command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1 \
         && have_rust_target x86_64-pc-windows-gnu; then
      rt="x86_64-pc-windows-gnu"
      info "빌드: cargo zigbuild --release --locked --target $rt"
      (cd "$ROOT" && cargo zigbuild --release --locked --target "$rt") || die "$rt 빌드 실패"
    elif command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 && have_rust_target x86_64-pc-windows-gnu; then
      rt="x86_64-pc-windows-gnu"
      info "빌드: cargo build --release --locked --target $rt (mingw-w64)"
      (cd "$ROOT" && CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
         CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
         cargo build --release --locked --target "$rt") || die "$rt 빌드 실패"
    else
      SKIP_REASON="이 PC에서는 Windows용으로 교차 빌드할 수 없습니다 (cargo-xwin, cargo-zigbuild+zig, mingw-w64 중 하나와 해당 Rust 타깃이 필요합니다. 파일 상단 설명 참고)"
      return 1
    fi
  fi

  local bin="$ROOT/target/$rt/release/gjc-connect$ext"
  [ -f "$bin" ] || die "빌드 결과를 찾을 수 없습니다: $bin"
  BUILT_BIN="$bin"
  return 0
}

# ---------------------------------------------------------------------------
# 묶음 조립
# ---------------------------------------------------------------------------
fetch_notices() {
  # 라이선스 고지는 최선 노력으로 받는다 (실패해도 묶음 생성은 계속하되 경고).
  local dest="$1" name
  mkdir -p "$dest"
  for name in LICENSE NOTICE.md; do
    if ! curl -fsSL --retry 2 --connect-timeout 15 \
         -o "$dest/$name.gajae-code" "https://raw.githubusercontent.com/$REPO/v$GJC_VERSION/$name"; then
      echo "경고: gajae-code $name 을(를) 받지 못했습니다. 반입 전에 직접 추가하세요" >&2
      rm -f "$dest/$name.gajae-code"
    fi
  done
}

assemble() {
  local label="$1" os="$2" asset="$3" ext="" stage name archive
  [ "$os" = windows ] && ext=".exe"
  name="gjc-connect-$label-$GJC_VERSION"
  stage="$OUT_DIR/stage/$name"
  rm -rf "$stage"
  mkdir -p "$stage/bin" "$stage/data" "$stage/licenses"

  cp "$BUILT_BIN" "$stage/gjc-connect$ext"
  cp "$CACHE_DIR/$asset" "$stage/bin/gjc$ext"
  chmod 755 "$stage/gjc-connect$ext" "$stage/bin/gjc$ext"
  cp "$ROOT/README.md" "$stage/README.md"
  : > "$stage/data/.keep"
  fetch_notices "$stage/licenses"

  if [ "$os" = windows ] && [ -n "$PORTABLE_GIT" ]; then
    mkdir -p "$stage/tools"
    if [ -d "$PORTABLE_GIT" ]; then
      [ -f "$PORTABLE_GIT/bin/bash.exe" ] || die "--portable-git 폴더에 bin/bash.exe가 없습니다: $PORTABLE_GIT"
      cp -R "$PORTABLE_GIT" "$stage/tools/PortableGit"
    elif [ -f "$PORTABLE_GIT" ]; then
      # PortableGit-*.7z.exe : 7z(7zz/7za)로 tools/PortableGit 에 푼다
      local sz=""
      for c in 7z 7zz 7za; do command -v "$c" >/dev/null 2>&1 && { sz="$c"; break; }; done
      [ -n "$sz" ] || die "PortableGit(.7z.exe)를 풀려면 7z(7zz/7za)가 필요합니다. 폴더로 풀어 --portable-git <폴더>로 지정해도 됩니다"
      mkdir -p "$stage/tools/PortableGit"
      "$sz" x -y -bso0 -bsp0 "-o$stage/tools/PortableGit" "$PORTABLE_GIT" || die "PortableGit 압축 해제 실패"
    else
      die "--portable-git 경로를 찾을 수 없습니다: $PORTABLE_GIT"
    fi
  fi

  if [ "$os" = windows ] && [ -n "$PORTABLE_GIT" ]; then
    [ -f "$stage/tools/PortableGit/bin/bash.exe" ] || die "PortableGit/bin/bash.exe를 찾을 수 없습니다"
    if [ -f "$stage/tools/PortableGit/LICENSE.txt" ]; then
      cp "$stage/tools/PortableGit/LICENSE.txt" "$stage/licenses/LICENSE.git-for-windows.txt"
    else
      echo "Git for Windows is GPLv2. Source: https://github.com/git-for-windows/git" > "$stage/licenses/LICENSE.git-for-windows.txt"
    fi
  fi

  local gjc_sha
  gjc_sha="$(sha256_of "$CACHE_DIR/$asset")"
  cat > "$stage/bundle.json" <<EOF
{
  "name": "$name",
  "target": "$label",
  "gjcVersion": "$GJC_VERSION",
  "gjcAsset": "$asset",
  "gjcSha256": "$gjc_sha",
  "gjcConnectVersion": "$(awk -F'"' '/^version *=/ {print $2; exit}' "$ROOT/Cargo.toml")",
  "builtAt": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

  if [ "$os" = windows ]; then
    command -v zip >/dev/null 2>&1 || die "Windows 묶음은 zip 명령이 필요합니다"
    archive="$OUT_DIR/$name.zip"
    rm -f "$archive"
    (cd "$OUT_DIR/stage" && zip -qr "$archive" "$name")
  else
    archive="$OUT_DIR/$name.tar.gz"
    rm -f "$archive"
    (cd "$OUT_DIR/stage" && tar -czf "$archive" "$name")
  fi
  info "생성: $archive"
  BUILT_ARCHIVES="$BUILT_ARCHIVES $archive"
}

# ---------------------------------------------------------------------------
# 메인
# ---------------------------------------------------------------------------
BUILT_ARCHIVES=""
SKIPPED=""
IFS=',' read -r -a TARGET_LIST <<< "$TARGETS"

if [ -n "$PORTABLE_GIT" ]; then
  case ",$TARGETS," in
    *,windows-x64,*) ;;
    *) echo "경고: --portable-git 은 windows-x64 대상에만 쓰입니다" >&2 ;;
  esac
fi

for t in "${TARGET_LIST[@]}"; do
  case "$t" in
    windows-x64) rust_target="x86_64-pc-windows-msvc"; os=windows; asset="gjc-windows-x64.exe" ;;
    macos-arm64) rust_target="aarch64-apple-darwin"; os=macos; asset="gjc-darwin-arm64" ;;
    macos-x64)   rust_target="x86_64-apple-darwin"; os=macos; asset="gjc-darwin-x64" ;;
    *) die "알 수 없는 대상: $t (windows-x64, macos-arm64, macos-x64 중에서 선택)" ;;
  esac
  echo
  info "대상: $t"
  fetch_gjc "$asset"

  if build_gjc_connect "$rust_target" "$os"; then
    assemble "$t" "$os" "$asset"
    # 호스트와 같은 macOS 대상이면 gjc 바이너리가 실제로 실행되는지 확인한다.
    if [ "$os" = macos ] && [ "$HOST_OS" = Darwin ]; then
      case "$t:$HOST_ARCH" in
        macos-arm64:arm64|macos-x64:x86_64)
          smoke_home="$(mktemp -d)"
          got="$(HOME="$smoke_home" "$OUT_DIR/stage/gjc-connect-$t-$GJC_VERSION/bin/gjc" --version 2>&1 || true)"
          rm -rf "$smoke_home"
          case "$got" in
            *"$GJC_VERSION"*) info "실행 확인: $got" ;;
            *) echo "경고: gjc --version 출력이 예상과 다릅니다: $got" >&2 ;;
          esac
          ;;
      esac
    fi
  else
    echo "건너뜀: $t — $SKIP_REASON" >&2
    SKIPPED="$SKIPPED $t"
  fi
done

echo
rm -rf "$OUT_DIR/stage"
if [ -n "$BUILT_ARCHIVES" ]; then
  : > "$OUT_DIR/SHA256SUMS"
  for a in $BUILT_ARCHIVES; do
    echo "$(sha256_of "$a")  $(basename "$a")" >> "$OUT_DIR/SHA256SUMS"
  done
  info "SHA256SUMS 생성: $OUT_DIR/SHA256SUMS"
  cat "$OUT_DIR/SHA256SUMS"
else
  echo "생성된 묶음이 없습니다." >&2
fi

if [ -n "$SKIPPED" ]; then
  echo >&2
  echo "요약: 건너뛴 대상 →$SKIPPED" >&2
  if [ "$STRICT" = 1 ]; then
    exit 3
  fi
fi
[ -n "$BUILT_ARCHIVES" ] || exit 2
exit 0
