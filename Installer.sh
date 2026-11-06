#!/usr/bin/env bash
# Qinari installer.
#
# Installs the qinari CLI from github.com/AlbiFahriiza/qinari: either a
# prebuilt release tarball, a release tag built from source, or the
# development HEAD of main. Run with --help for options and exit codes.

set -euo pipefail

readonly REPO="AlbiFahriiza/qinari"
readonly INSTALLER_VERSION="1.0.0"
readonly ARCHIVE_NAME="qinari-x86_64-unknown-linux-gnu"
readonly RELEASES_URL_BASE="https://github.com/${REPO}/releases/download"

TMP_DIR=""

usage() {
  cat <<'EOF'
Qinari installer

Installs the qinari CLI from github.com/AlbiFahriiza/qinari, either as a
prebuilt binary or built from source.

Usage:
  Installer.sh [OPTIONS]

Options:
  --tag <tag>      Install or build this release tag instead of the
                   latest published release.
  --prefix <dir>   Install under this prefix (default: ~/.local). The
                   binary lands in <dir>/bin/qinari.
  --build          Build from source with cargo at the selected tag
                   (default: latest release) instead of downloading the
                   prebuilt tarball. Requires git, rustup, and cargo.
  --dev            Clone and build the development HEAD of main.
                   Requires git, rustup, and cargo.
  --uninstall      Remove the installed binary from <prefix>/bin.
  --version        Print the installer version and exit.
  --help           Print this help and exit.

Exit codes:
  0                Success.
  1                Generic failure (environment or install step).
  2                Usage error (unknown option, missing or conflicting
                   option values).
  3                Download, checksum, or release artifact failure.
  4                Build failure (missing build tools or cargo failed).
  5                Uninstall found no qinari binary to remove.
EOF
}

die() {
  local code="$1"
  shift
  printf 'Installer.sh: %s\n' "$*" >&2
  exit "${code}"
}

usage_error() {
  printf 'Installer.sh: %s\n\n' "$*" >&2
  usage >&2
  exit 2
}

cleanup() {
  # The EXIT trap wipes the staging directory so a failed run leaves no
  # .part files or half-extracted tree behind.
  if [[ -n "${TMP_DIR}" && -d "${TMP_DIR}" ]]; then
    rm -rf -- "${TMP_DIR}"
  fi
}
trap cleanup EXIT

fetch() {
  # Same curl flag set as the qinari CLI's own downloader, so both
  # retry and fail identically. wget is never used.
  local url="$1" dest="$2"
  curl --fail --location --silent --show-error --retry 3 \
    --output "${dest}" "${url}"
}

resolve_latest_tag() {
  local body tag
  # /releases/latest skips prereleases and this project ships alpha
  # tags, so ask for the newest release of any kind instead.
  if ! body="$(curl --fail --location --silent --show-error --retry 3 \
    "https://api.github.com/repos/${REPO}/releases?per_page=1")"; then
    return 1
  fi
  tag="$(printf '%s\n' "${body}" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
  if [[ -z "${tag}" ]]; then
    return 1
  fi
  printf '%s\n' "${tag}"
}

install_binary() {
  local src="$1"
  mkdir -p "${PREFIX}/bin"
  # Copy to a temporary name in the destination directory and rename it
  # into place: the rename is atomic, so a qinari process started at the
  # wrong moment never sees a truncated binary.
  if ! install -m 0755 "${src}" "${PREFIX}/bin/qinari.part"; then
    rm -f -- "${PREFIX}/bin/qinari.part"
    die 1 "failed to write ${PREFIX}/bin/qinari"
  fi
  mv -f "${PREFIX}/bin/qinari.part" "${PREFIX}/bin/qinari"
}

warn_if_bin_not_on_path() {
  case ":${PATH}:" in
    *":${PREFIX}/bin:"*) ;;
    *)
      printf 'Installer.sh: warning: %s is not on your PATH; add it to run qinari by name.\n' \
        "${PREFIX}/bin" >&2
      ;;
  esac
}

install_release_tarball() {
  local tarball_url="${RELEASES_URL_BASE}/${TAG}/${ARCHIVE_NAME}.tar.gz"
  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/qinari-install.XXXXXXXX")"

  printf 'Downloading qinari %s ...\n' "${TAG}"
  if ! fetch "${tarball_url}" "${TMP_DIR}/${ARCHIVE_NAME}.tar.gz.part"; then
    die 3 "download failed: ${tarball_url}"
  fi
  if ! fetch "${tarball_url}.sha256" "${TMP_DIR}/${ARCHIVE_NAME}.tar.gz.sha256.part"; then
    die 3 "download failed: ${tarball_url}.sha256"
  fi

  # Both parts are fully downloaded: give them their final names inside
  # the private staging directory, then verify. Nothing is written into
  # PREFIX until the checksum has passed.
  mv "${TMP_DIR}/${ARCHIVE_NAME}.tar.gz.sha256.part" \
    "${TMP_DIR}/${ARCHIVE_NAME}.tar.gz.sha256"
  mv "${TMP_DIR}/${ARCHIVE_NAME}.tar.gz.part" \
    "${TMP_DIR}/${ARCHIVE_NAME}.tar.gz"
  if ! (cd "${TMP_DIR}" && sha256sum --check --status \
    "${ARCHIVE_NAME}.tar.gz.sha256"); then
    die 3 "checksum mismatch for ${ARCHIVE_NAME}.tar.gz"
  fi
  printf 'Checksum verified.\n'

  if ! tar -xzf "${TMP_DIR}/${ARCHIVE_NAME}.tar.gz" -C "${TMP_DIR}"; then
    die 3 "failed to extract ${ARCHIVE_NAME}.tar.gz"
  fi
  if [[ ! -f "${TMP_DIR}/qinari" ]]; then
    die 3 "tarball does not contain a qinari binary"
  fi

  install_binary "${TMP_DIR}/qinari"
  printf 'Installed qinari %s to %s\n' "${TAG}" "${PREFIX}/bin/qinari"
  warn_if_bin_not_on_path
}

build_from_source() {
  local ref="$1"
  # rustup is required because the clone's rust-toolchain.toml picks the
  # pinned toolchain automatically during cargo build.
  command -v git >/dev/null 2>&1 || die 4 "git is required to fetch the sources"
  command -v rustup >/dev/null 2>&1 || die 4 "rustup is required to build from source"
  command -v cargo >/dev/null 2>&1 || die 4 "cargo is required to build from source"

  TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/qinari-build.XXXXXXXX")"
  printf 'Cloning qinari %s ...\n' "${ref}"
  if ! git clone --depth 1 --branch "${ref}" \
    "https://github.com/${REPO}.git" "${TMP_DIR}/src"; then
    die 4 "git clone failed for ${ref}"
  fi

  printf 'Building qinari %s (this can take several minutes) ...\n' "${ref}"
  if ! (cd "${TMP_DIR}/src" && cargo build --release --bin qinari); then
    die 4 "cargo build failed for ${ref}"
  fi
  if [[ ! -f "${TMP_DIR}/src/target/release/qinari" ]]; then
    die 4 "cargo build produced no qinari binary"
  fi

  install_binary "${TMP_DIR}/src/target/release/qinari"
  printf 'Built and installed qinari %s to %s\n' "${ref}" "${PREFIX}/bin/qinari"
  warn_if_bin_not_on_path
}

uninstall_qinari() {
  local target="${PREFIX}/bin/qinari"
  if [[ ! -e "${target}" && ! -L "${target}" ]]; then
    die 5 "no qinari binary found at ${target}"
  fi
  if ! rm -f -- "${target}"; then
    die 1 "failed to remove ${target}"
  fi
  printf 'Removed %s\n' "${target}"
}

require_value() {
  local value="$1" option="$2"
  if [[ -z "${value}" || "${value}" == -* ]]; then
    usage_error "${option} requires a value"
  fi
}

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --tag)
        [[ $# -ge 2 ]] || usage_error "--tag requires a value"
        require_value "$2" "--tag"
        TAG="$2"
        shift 2
        ;;
      --tag=*)
        TAG="${1#*=}"
        require_value "${TAG}" "--tag"
        shift
        ;;
      --prefix)
        [[ $# -ge 2 ]] || usage_error "--prefix requires a value"
        require_value "$2" "--prefix"
        PREFIX="$2"
        shift 2
        ;;
      --prefix=*)
        PREFIX="${1#*=}"
        require_value "${PREFIX}" "--prefix"
        shift
        ;;
      --build)
        WANT_BUILD=true
        shift
        ;;
      --dev)
        WANT_DEV=true
        shift
        ;;
      --uninstall)
        WANT_UNINSTALL=true
        shift
        ;;
      --version)
        printf 'Installer.sh %s (qinari installer for github.com/%s)\n' \
          "${INSTALLER_VERSION}" "${REPO}"
        exit 0
        ;;
      --help)
        usage
        exit 0
        ;;
      *)
        usage_error "unknown option: $1"
        ;;
    esac
  done
}

validate_options() {
  if [[ "${WANT_UNINSTALL}" == true ]]; then
    if [[ "${WANT_BUILD}" == true || "${WANT_DEV}" == true || -n "${TAG}" ]]; then
      usage_error "--uninstall cannot be combined with --tag, --build, or --dev"
    fi
  fi
  if [[ "${WANT_DEV}" == true ]]; then
    if [[ -n "${TAG}" ]]; then
      usage_error "--dev builds main and cannot be combined with --tag"
    fi
    if [[ "${WANT_BUILD}" == true ]]; then
      usage_error "--build and --dev are mutually exclusive"
    fi
  fi

  if [[ -z "${PREFIX}" ]]; then
    if [[ -z "${HOME:-}" ]]; then
      die 1 "HOME is not set; pass --prefix <dir>"
    fi
    PREFIX="${HOME}/.local"
  fi
  # Normalize the prefix so PREFIX/bin can be matched against PATH
  # entries verbatim and never ends up pointing at the filesystem root.
  while [[ "${PREFIX}" == */ && "${PREFIX}" != "/" ]]; do
    PREFIX="${PREFIX%/}"
  done
  if [[ "${PREFIX}" != /* ]]; then
    PREFIX="${PWD}/${PREFIX}"
  fi
}

ensure_tag() {
  if [[ -n "${TAG}" ]]; then
    return 0
  fi
  local latest
  printf 'Resolving the latest release ...\n'
  if ! latest="$(resolve_latest_tag)"; then
    die 3 "could not resolve the latest release of ${REPO} (no releases published yet, or the GitHub API is unreachable)"
  fi
  TAG="${latest}"
}

main() {
  parse_args "$@"
  validate_options

  if [[ "${WANT_UNINSTALL}" == true ]]; then
    uninstall_qinari
    exit 0
  fi
  if [[ "${WANT_DEV}" == true ]]; then
    build_from_source "main"
    exit 0
  fi
  if [[ "${WANT_BUILD}" == true ]]; then
    ensure_tag
    build_from_source "${TAG}"
    exit 0
  fi
  ensure_tag
  install_release_tarball
}

TAG=""
PREFIX=""
WANT_BUILD=false
WANT_DEV=false
WANT_UNINSTALL=false

main "$@"
