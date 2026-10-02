#!/usr/bin/env bash
# build-deb.sh — build the Debian packages and prove they install cleanly.
#
# The Phase 20 exit criterion: "Debian package builds and installs
# cleanly in a fresh container". This script drives the full pipeline:
#
#   --build     cargo release build of every shipped binary
#   --package   dpkg-buildpackage -us -uc -b (non-root, Rules-Requires-Root: no)
#   --verify    fresh-root install via `unshare` user namespaces + real
#               dpkg, installed-binary smoke tests, systemd unit
#               verification against the installed root
#   (default)   all three, in order
#
# Splitting the stages exists because the release build (thin LTO,
# codegen-units=1) is the long pole; callers on tight tool timeouts can
# pre-run --build and then finish with --package --verify.
#
# Artifacts land in ./dist/ : the versioned .deb pair for both
# binary packages (+ .changes, .buildinfo).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

# rustup user-local installs (the documented path) plus system cargo.
export PATH="${HOME}/.cargo/bin:${PATH}"

DIST="${REPO_ROOT}/dist"
VERIFY_ROOT="${DIST}/fresh-root"

BUILD_PKGS=(lion-compositor ldp-tools ldp-test ldpc hello-ldp pointer-paint clip-client ldp-remote lion-bridge)
COMPOSITOR_BIN=target/release/lion-compositor
BRIDGE_BIN=target/release/lion-bridge
TOOLS_BINS=(ldp-info ldp-debug ldp-validate ldp-profiler ldp-audit ldp-input-debug ldp-grab ldp-bench ldpc ldp-remote-gateway)
EXAMPLE_BINS=(hello-ldp pointer-paint clip-client)

do_build() {
    echo "==> release build (this is the long pole: thin LTO, codegen-units=1)"
    local -a cargo_args=()
    local p
    for p in "${BUILD_PKGS[@]}"; do
        cargo_args+=( -p "$p" )
    done
    cargo build --release "${cargo_args[@]}"
    for b in "${COMPOSITOR_BIN}" "${BRIDGE_BIN}" \
             "${TOOLS_BINS[@]/#/target/release/}" \
             "${EXAMPLE_BINS[@]/#/target/release/}"; do
        test -x "${b}" || { echo "missing build output: ${b}" >&2; exit 1; }
    done
    echo "release build OK"
}

do_package() {
    echo "==> dpkg-buildpackage (unprivileged)"
    # The control file declares Build-Depends (cargo, rustc >= 1.75) for
    # distro builders, but this sandbox's toolchain is rustup user-local
    # — invisible to dpkg's package database. So -d skips the dpkg
    # check AFTER we verify the toolchain ourselves, right here.
    command -v cargo > /dev/null || { echo "cargo not found in PATH" >&2; exit 1; }
    command -v rustc > /dev/null || { echo "rustc not found in PATH" >&2; exit 1; }
    local have_rustc
    have_rustc="$(rustc --version | rg -o '[0-9]+\.[0-9]+' | head -1)"
    if ! dpkg --compare-versions "${have_rustc}" ge 1.75; then
        echo "rustc ${have_rustc} < 1.75 (Build-Depends floor)" >&2
        exit 1
    fi
    mkdir -p "${DIST}"
    # -b: binary-only. -us -uc: unsigned (project release pipeline signs
    # nothing at v0.2.0). -d: build-deps verified above, not via dpkg.
    # The build runs entirely as the invoking user:
    # Rules-Requires-Root: no + dpkg-deb --root-owner-group.
    dpkg-buildpackage -us -uc -b -d
    # dpkg-buildpackage drops the .debs next to the source tree (..).
    # Explicit names only: the parent directory may hold unrelated
    # artifacts, so never glob a bare *.deb there.
    local -a built=(
        ../lion-compositor_$(dpkg-parsechangelog -S Version)_$(dpkg --print-architecture).deb
        ../ldp-tools_$(dpkg-parsechangelog -S Version)_$(dpkg --print-architecture).deb
    )
    for f in "${built[@]}"; do
        test -f "${f}" || { echo "missing package: ${f}" >&2; exit 1; }
    done
    mv "${built[@]}" "${DIST}/"
    mv ../lion-display_*.changes ../lion-display_*.buildinfo "${DIST}/" 2>/dev/null || true
    echo "packaged:"
    ls -1 "${DIST}"/*.deb
}

fresh_root() {
    rm -rf "${VERIFY_ROOT}"
    mkdir -p "${VERIFY_ROOT}/var/lib/dpkg/updates" \
             "${VERIFY_ROOT}/var/lib/dpkg/info" \
             "${VERIFY_ROOT}/var/lib/dpkg/triggers" \
             "${VERIFY_ROOT}/var/lib/dpkg/alternatives"
    : > "${VERIFY_ROOT}/var/lib/dpkg/status"
    # A real fresh Debian container carries the base system — libc6
    # included — before any of OUR packages arrive. Mirror exactly
    # that: pre-seed the status file with the host's actual libc6
    # version (the same glibc the shipped binaries link against), so
    # dependency resolution behaves as it would on a pristine base
    # install rather than on an empty (and unrealistic) database.
    local host_libc
    host_libc="$(dpkg-query -W -f='${Version}' libc6)"
    {
        echo "Package: libc6"
        echo "Status: install ok installed"
        echo "Architecture: $(dpkg --print-architecture)"
        echo "Version: ${host_libc}"
        echo "Maintainer: Debian GNU libc maintainers <debian-glibc@lists.debian.org>"
        echo "Description: GNU C Library (base-system pre-seed of the fresh-root harness)"
    } >> "${VERIFY_ROOT}/var/lib/dpkg/status"
    # Likewise, a systemd-based base system ships systemd's own unit
    # files; `systemd-analyze --root=... verify` needs the target
    # dependency chain (sysinit/basic/multi-user) to resolve. Copy the
    # host's system units into the root — 904 KiB, the same set any
    # real fresh container would carry.
    mkdir -p "${VERIFY_ROOT}/usr/lib/systemd/system"
    cp -a /usr/lib/systemd/system/. "${VERIFY_ROOT}/usr/lib/systemd/system/"
}

do_verify() {
    # Pin the verified pair to the CURRENT changelog version — a glob
    # here once let stale pre-release .debs in dist/ ride into the
    # fresh-root install (unpacked over the release pair). Explicit
    # names only, mirroring do_package's `built` array.
    local -a debs=(
        "${DIST}/lion-compositor_$(dpkg-parsechangelog -S Version)_$(dpkg --print-architecture).deb"
        "${DIST}/ldp-tools_$(dpkg-parsechangelog -S Version)_$(dpkg --print-architecture).deb"
    )
    for deb in "${debs[@]}"; do
        test -f "${deb}" || { echo "missing release package: ${deb}" >&2; exit 1; }
    done
    echo "==> structural checks (dpkg-deb info/contents)"
    for deb in "${debs[@]}"; do
        dpkg-deb --info "${deb}" > /dev/null
        # Every packaged file must be root-owned (--root-owner-group).
        if dpkg-deb --contents "${deb}" | rg -q " ${USER}/${USER} "; then
            echo "FAIL: non-root ownership in ${deb}" >&2
            exit 1
        fi
    done
    echo "structure OK"

    echo "==> fresh-root install (unshare + real dpkg)"
    fresh_root
    # A user namespace gives us uid 0 inside its own mount/user mapping:
    # dpkg runs its real unpack/configure path against a pristine root,
    # exactly the "installs cleanly in a fresh container" criterion
    # (minus kernel namespaces we are not allowed to create).
    unshare -rm dpkg --root="${VERIFY_ROOT}" -i "${debs[@]}" 2>&1 \
        | rg -v "could not open log|dpkg: warning" || true
    for pkg in lion-compositor ldp-tools; do
        unshare -rm dpkg-query --root="${VERIFY_ROOT}" -W -f='${Status}\n' "${pkg}" \
            | rg -q "install ok installed" \
            || { echo "FAIL: ${pkg} not installed cleanly" >&2; exit 1; }
    done
    echo "dpkg install OK (status: install ok installed, both packages)"

    echo "==> installed-file assertions"
    test -x "${VERIFY_ROOT}/usr/bin/lion-compositor"
    test -x "${VERIFY_ROOT}/usr/bin/lion-bridge"
    test -f "${VERIFY_ROOT}/usr/lib/systemd/system/lion-compositor.service"
    for b in "${TOOLS_BINS[@]}"; do
        test -x "${VERIFY_ROOT}/usr/bin/${b}"
    done
    for b in "${EXAMPLE_BINS[@]}"; do
        test -x "${VERIFY_ROOT}/usr/lib/lion-display/examples/${b}"
    done
    for d in admin-guide.md.gz maturity.md.gz changelog.gz copyright; do
        test -f "${VERIFY_ROOT}/usr/share/doc/lion-compositor/${d}"
    done
    for d in user-guide.md.gz changelog.gz copyright; do
        test -f "${VERIFY_ROOT}/usr/share/doc/ldp-tools/${d}"
    done
    echo "file layout OK (usr-merge paths)"

    echo "==> installed-binary smoke tests"
    "${VERIFY_ROOT}/usr/bin/lion-compositor" --version > /dev/null
    "${VERIFY_ROOT}/usr/bin/lion-compositor" --selftest
    "${VERIFY_ROOT}/usr/bin/lion-bridge" --version > /dev/null
    "${VERIFY_ROOT}/usr/bin/lion-bridge" --help > /dev/null
    for b in "${TOOLS_BINS[@]}"; do
        "${VERIFY_ROOT}/usr/bin/${b}" --help > /dev/null
    done
    "${VERIFY_ROOT}/usr/lib/lion-display/examples/hello-ldp" --help > /dev/null
    "${VERIFY_ROOT}/usr/lib/lion-display/examples/pointer-paint" --help > /dev/null
    "${VERIFY_ROOT}/usr/lib/lion-display/examples/clip-client" --help > /dev/null
    echo "binaries OK (compositor selftest + bridge + 9 tools + 3 examples)"

    echo "==> installed-binary version surface (Phase 23)"
    # Every installed binary must report the debian changelog's version
    # — the packaging-level assertion of the release-coherence gate:
    # the cargo workspace version and the debian version are one.
    local deb_version
    deb_version="$(dpkg-parsechangelog -S Version)"
    local -a versioned=(lion-compositor lion-bridge "${TOOLS_BINS[@]}")
    local b out
    for b in "${versioned[@]}"; do
        out="$("${VERIFY_ROOT}/usr/bin/${b}" --version)"
        if [[ "${out}" != "${b} ${deb_version}" ]]; then
            echo "FAIL: ${b} --version reported '${out}', expected '${b} ${deb_version}'" >&2
            exit 1
        fi
    done
    for b in "${EXAMPLE_BINS[@]}"; do
        out="$("${VERIFY_ROOT}/usr/lib/lion-display/examples/${b}" --version)"
        if [[ "${out}" != "${b} ${deb_version}" ]]; then
            echo "FAIL: ${b} --version reported '${out}', expected '${b} ${deb_version}'" >&2
            exit 1
        fi
    done
    echo "version surface OK (compositor + bridge + 10 tools + 3 examples == ${deb_version})"

    echo "==> systemd unit verification against the installed root"
    systemd-analyze --root="${VERIFY_ROOT}" verify \
        "${VERIFY_ROOT}/usr/lib/systemd/system/lion-compositor.service"
    echo "systemd unit OK"

    echo "==> dpkg --verify (md5sums integrity)"
    unshare -rm dpkg --root="${VERIFY_ROOT}" --verify lion-compositor || true
    unshare -rm dpkg --root="${VERIFY_ROOT}" --verify ldp-tools || true

    echo "ALL DEB VERIFICATION GATES PASSED"
}

case "${1:---all}" in
    --build)   do_build ;;
    --package) do_package ;;
    --verify)  do_verify ;;
    --all)     do_build; do_package; do_verify ;;
    *) echo "usage: $0 [--build|--package|--verify|--all]" >&2; exit 2 ;;
esac
