#!/usr/bin/env sh
#
# Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
#
# Install public Volcengine storage CLI release binaries from GitHub Releases.
#
# Usage:
#   curl -fsSL https://.../install.sh | sh
#   curl -fsSL https://.../install.sh | sh -s -- ve-tos-cli
#   curl -fsSL https://.../install.sh | sh -s -- tos-cli
#   VE_STORAGE_UNI_CLI_VERSION=v1.0.0 sh install.sh ve-adrive-cli

set -eu

REPO_URL="${VE_STORAGE_UNI_CLI_REPO_URL:-https://github.com/volcengine/ve-storage-uni-cli}"
VERSION="${VE_STORAGE_UNI_CLI_VERSION:-latest}"
INSTALL_DIR="${VE_STORAGE_UNI_CLI_INSTALL_DIR:-$HOME/.local/bin}"
REQUESTED_COMMAND="${1:-all}"

die() {
  printf '%s\n' "error: $*" >&2
  exit 1
}

info() {
  printf '%s\n' "$*" >&2
}

need_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command '$1' was not found"
}

normalize_os() {
  case "$(uname -s)" in
    Darwin) printf '%s' "apple-darwin" ;;
    Linux) printf '%s' "unknown-linux" ;;
    MINGW*|MSYS*|CYGWIN*) printf '%s' "pc-windows-msvc" ;;
    *) die "unsupported OS: $(uname -s)" ;;
  esac
}

normalize_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf '%s' "x86_64" ;;
    arm64|aarch64) printf '%s' "aarch64" ;;
    *) die "unsupported architecture: $(uname -m)" ;;
  esac
}

detect_libc() {
  if [ "$(normalize_os)" != "unknown-linux" ]; then
    return 0
  fi
  if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
    printf '%s' "-musl"
  else
    printf '%s' "-gnu"
  fi
}

target_triple() {
  os="$(normalize_os)"
  arch="$(normalize_arch)"
  case "$os" in
    apple-darwin) printf '%s' "$arch-apple-darwin" ;;
    unknown-linux) printf '%s' "$arch-unknown-linux$(detect_libc)" ;;
    pc-windows-msvc) printf '%s' "$arch-pc-windows-msvc" ;;
    *) die "unsupported target OS: $os" ;;
  esac
}

archive_name() {
  triple="$1"
  case "$triple" in
    *windows*) printf '%s' "ve-storage-uni-cli-$triple.zip" ;;
    *) printf '%s' "ve-storage-uni-cli-$triple.tar.gz" ;;
  esac
}

asset_base_url() {
  if [ "$VERSION" = "latest" ]; then
    printf '%s' "$REPO_URL/releases/latest/download"
  else
    printf '%s' "$REPO_URL/releases/download/$VERSION"
  fi
}

download_file() {
  url="$1"
  dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$dest"
  else
    die "curl or wget is required to download release assets"
  fi
}

verify_checksum() {
  archive="$1"
  archive_path="$2"
  sums_path="$3"
  expected="$(grep " $archive\$" "$sums_path" | awk '{print $1}')"
  [ -n "$expected" ] || die "no SHA256 entry found for $archive"

  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$archive_path" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$archive_path" | awk '{print $1}')"
  else
    die "sha256sum or shasum is required to verify release assets"
  fi

  [ "$actual" = "$expected" ] || die "checksum mismatch for $archive"
}

extract_archive() {
  archive_path="$1"
  dest_dir="$2"
  case "$archive_path" in
    *.zip)
      need_command unzip
      unzip -q "$archive_path" -d "$dest_dir"
      ;;
    *.tar.gz)
      need_command tar
      tar -xzf "$archive_path" -C "$dest_dir"
      ;;
    *)
      die "unsupported archive format: $archive_path"
      ;;
  esac
}

install_command() {
  command_name="$1"
  extracted_dir="$2"
  mkdir -p "$INSTALL_DIR"

  src="$extracted_dir/bin/$command_name"
  if [ -f "$src.exe" ]; then
    src="$src.exe"
  fi
  [ -f "$src" ] || die "archive did not contain bin/$command_name"

  dest="$INSTALL_DIR/$command_name"
  if [ "${src##*.}" = "exe" ]; then
    dest="$dest.exe"
  fi

  cp "$src" "$dest"
  chmod 0755 "$dest"
  info "installed $command_name to $dest"
}

commands_to_install() {
  case "$REQUESTED_COMMAND" in
    all|"")
      printf '%s\n' ve-tos-cli tos-cli ve-adrive-cli
      ;;
    ve-tos-cli|tos-cli|ve-adrive-cli)
      printf '%s\n' "$REQUESTED_COMMAND"
      ;;
    *)
      die "expected one of: all, ve-tos-cli, tos-cli, ve-adrive-cli"
      ;;
  esac
}

main() {
  triple="$(target_triple)"
  archive="$(archive_name "$triple")"
  base_url="$(asset_base_url)"
  temp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t ve-storage-uni-cli)"
  # [Review Fix #2] Remove downloaded archives after install so curl-based
  # installs do not leave verified binaries in a temporary directory.
  trap 'rm -rf "$temp_dir"' EXIT HUP INT TERM

  info "downloading $archive"
  download_file "$base_url/$archive" "$temp_dir/$archive"
  download_file "$base_url/SHA256SUMS" "$temp_dir/SHA256SUMS"
  verify_checksum "$archive" "$temp_dir/$archive" "$temp_dir/SHA256SUMS"
  extract_archive "$temp_dir/$archive" "$temp_dir/extract"

  commands_to_install | while IFS= read -r command_name; do
    install_command "$command_name" "$temp_dir/extract"
  done

  info "done"
}

main
