#!/bin/sh
# Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
# SPDX-License-Identifier: Apache-2.0
# The release script fills the HTTPS base; users must select an explicit version.

set -eu

DEFAULT_DOWNLOAD_BASE_URL=''
BASE_URL="${TOS_CLI_DOWNLOAD_BASE_URL:-$DEFAULT_DOWNLOAD_BASE_URL}"
INSTALL_DIR="${TOS_CLI_INSTALL_DIR:-$HOME/.local/bin}"
VERSION=''
WORK_DIR=''
STAGED_BINARY=''

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [ -n "$WORK_DIR" ]; then rm -rf "$WORK_DIR"; fi
  if [ -n "$STAGED_BINARY" ]; then rm -f "$STAGED_BINARY"; fi
}

parse_options() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --version|--install-dir)
        [ "$#" -ge 2 ] || die "$1 requires a value"
        case "$1" in
          --version) VERSION="$2" ;;
          --install-dir) INSTALL_DIR="$2" ;;
        esac
        shift 2
        ;;
      --help|-h)
        printf 'Usage: sh install.sh --version <version> [--install-dir <directory>]\n'
        exit 0
        ;;
      *) die "unknown argument: $1" ;;
    esac
  done
  [ -n "$VERSION" ] || die '--version is required'
  case "$VERSION" in *[!A-Za-z0-9.+-]*) die 'invalid version' ;; esac
  case "$VERSION" in [0-9]*.[0-9]*.[0-9]*) ;; *) die 'invalid version' ;; esac
  case "$BASE_URL" in https://?*) ;; *) die 'an HTTPS download base URL is required' ;; esac
  [ -n "$INSTALL_DIR" ] || die 'installation directory cannot be empty'
  case "$INSTALL_DIR" in /*) ;; *) INSTALL_DIR="$PWD/$INSTALL_DIR" ;; esac
  BASE_URL="${BASE_URL%/}"
}

target_triple() {
  case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=aarch64 ;;
    *) die 'unsupported architecture' ;;
  esac
  case "$(uname -s)" in
    Darwin) printf '%s-apple-darwin' "$arch" ;;
    Linux)
      if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
        die 'musl Linux is not supported'
      fi
      printf '%s-unknown-linux-gnu.2.17' "$arch"
      ;;
    MINGW*|MSYS*|CYGWIN*)
      [ "$arch" = x86_64 ] || die 'unsupported Windows architecture'
      printf 'x86_64-pc-windows-msvc'
      ;;
    *) die 'unsupported OS' ;;
  esac
}

download() {
  if command -v curl >/dev/null 2>&1; then
    # [Review Fix #InternalHttpsRedirect] Keep redirected downloads on HTTPS.
    curl -fsSL --proto '=https' --proto-redir '=https' \
      --connect-timeout 15 --max-time 300 --retry 2 "$1" -o "$2"
  else
    die 'curl is required'
  fi
}

verify_archive() {
  expected="$(awk -v name="$ARCHIVE" '$2 == name { print $1 }' "$WORK_DIR/SHA256SUMS")"
  [ "${#expected}" -eq 64 ] || die 'missing or duplicate archive checksum'
  case "$expected" in *[!a-f0-9]*) die 'invalid archive checksum' ;; esac
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$WORK_DIR/$ARCHIVE" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$WORK_DIR/$ARCHIVE" | awk '{print $1}')"
  else
    die 'sha256sum or shasum is required'
  fi
  [ "$actual" = "$expected" ] || die 'archive checksum mismatch'
}

install_binary() {
  case "$ARCHIVE" in
    *.zip)
      command -v unzip >/dev/null 2>&1 || die 'unzip is required'
      unzip -p "$WORK_DIR/$ARCHIVE" bin/tos-cli.exe > "$WORK_DIR/binary"
      binary_name=tos-cli.exe
      ;;
    *)
      tar -xOzf "$WORK_DIR/$ARCHIVE" bin/tos-cli > "$WORK_DIR/binary"
      binary_name=tos-cli
      ;;
  esac
  [ -s "$WORK_DIR/binary" ] || die 'archive has no tos-cli binary'
  mkdir -p "$INSTALL_DIR"
  # [Review Fix #InternalInstallAtomic] Verify and stage before replacing an
  # existing executable, including when the old destination is a symlink.
  STAGED_BINARY="$(mktemp "$INSTALL_DIR/.tos-cli-install.XXXXXX")"
  cp "$WORK_DIR/binary" "$STAGED_BINARY"
  chmod 0755 "$STAGED_BINARY"
  mv -f "$STAGED_BINARY" "$INSTALL_DIR/$binary_name"
  STAGED_BINARY=''
  printf 'Installed %s to %s; ensure this directory is in PATH.\n' "$binary_name" "$INSTALL_DIR" >&2
}

main() {
  parse_options "$@"
  triple="$(target_triple)"
  case "$triple" in *windows-msvc) suffix=zip ;; *) suffix=tar.gz ;; esac
  ARCHIVE="tos-cli-$VERSION-$triple.$suffix"
  WORK_DIR="$(mktemp -d)"
  trap cleanup EXIT
  trap 'exit 1' HUP INT TERM
  download "$BASE_URL/$VERSION/$ARCHIVE" "$WORK_DIR/$ARCHIVE"
  download "$BASE_URL/$VERSION/SHA256SUMS" "$WORK_DIR/SHA256SUMS"
  verify_archive
  install_binary
}

main "$@"
