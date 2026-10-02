#!/usr/bin/env bash
# package-phase.sh — build the phase-N zip deliverable.
#
# Usage: ./scripts/package-phase.sh [phase-number]
# Phase number defaults to the highest entry in CHANGELOG.md.
#
# Produces phase<N>.zip in the repository parent's download/ directory
# (or ./dist when run outside the build sandbox). The archive contains the
# repository tree excluding build outputs and VCS internals.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGELOG="${REPO_ROOT}/CHANGELOG.md"

if [[ -n "${1:-}" ]]; then
    PHASE="${1}"
else
    PHASE="$(grep -oE '0\.1\.0-phase[0-9]+' "${CHANGELOG}" | head -1 | grep -oE '[0-9]+$')"
fi
if [[ -z "${PHASE}" ]]; then
    echo "error: cannot determine phase number; pass it explicitly" >&2
    exit 1
fi

OUT_DIR="${LDP_DOWNLOAD_DIR:-/home/z/my-project/download}"
if ! mkdir -p "${OUT_DIR}"; then
    OUT_DIR="${REPO_ROOT}/dist"
    mkdir -p "${OUT_DIR}"
fi
OUT="${OUT_DIR}/phase${PHASE}.zip"

STAGING="$(mktemp -d)"
trap 'rm -rf "${STAGING}"' EXIT

# Use git archive only when the repo root itself is the work tree; an
# ancestor repository (sandbox checkouts) must not hijack the archive.
GIT_TOPLEVEL="$(git -C "${REPO_ROOT}" rev-parse --show-toplevel 2>/dev/null || true)"
if [[ "${GIT_TOPLEVEL}" == "${REPO_ROOT}" ]]; then
    git -C "${REPO_ROOT}" archive --format=tar --prefix="lion-display/" HEAD \
        | tar -x -C "${STAGING}"
else
    # Plain-tree fallback with the standard exclusions.
    mkdir -p "${STAGING}/lion-display"
    tar -C "${REPO_ROOT}" \
        --exclude='./target' --exclude='./.git' --exclude='./dist' \
        --exclude='./fuzz/corpus' --exclude='./fuzz/artifacts' \
        --exclude='./packaging/build' \
        -cf - . | tar -x -C "${STAGING}/lion-display"
fi

( cd "${STAGING}" && zip -qr9 "${OUT}" lion-display )

echo "packaged: ${OUT}"
unzip -l "${OUT}" | tail -3
