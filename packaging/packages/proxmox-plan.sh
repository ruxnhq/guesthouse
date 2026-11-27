#!/bin/sh
# Says what the Proxmox workflow builds, and whether it publishes it, before the two-hour build. A
# version that's in the testing or stable suite already is never built again for them: what's
# published never changes, and apt would find different files under one version.
#
# Usage: packaging/packages/proxmox-plan.sh EVENT REF SUFFIX
#
# EVENT is the workflow's event. REF is what to build of pve-qemu.git, empty for the newest Proxmox
# has, and SUFFIX is what goes after Proxmox's version. It writes ref, version, build and publish to
# $GITHUB_OUTPUT, or prints them with no GITHUB_OUTPUT. version is empty when REF is a commit or a
# branch, and the publish job checks the package's own version then.
set -eu

PROXMOX_PACKAGES=http://download.proxmox.com/debian/pve/dists/trixie/pve-no-subscription/binary-amd64/Packages.gz
OURS=${PROXMOX_REPO_URL:-https://pkg.ruxn.com/proxmox}
SUITES='testing stable'

die() {
    printf 'proxmox-plan: %s\n' "$*" >&2
    exit 1
}

# The newest pve-qemu-kvm in an index. Proxmox's isn't signed this way round, but it only picks what
# to build: the build itself checks Proxmox's key.
newest_in() {
    awk '/^Package: / { name = $2 } /^Version: / && name == "pve-qemu-kvm" { print $2 }' | sort -V | tail -n 1
}

published() {
    for suite in $SUITES; do
        curl -fsSL --max-time 30 "$OURS/dists/$suite/main/binary-amd64/Packages" 2>/dev/null |
            awk '/^Package: / { name = $2 } /^Version: / && name == "pve-qemu-kvm" { print $2 }' || true
    done
}

event=${1:?name the event}
ref=${2:-}
suffix=${3:-+gpupv1}
case $suffix in
    +[a-z0-9]*) ;;
    *) die "the suffix has to start with + and a letter or a digit, as $suffix doesn't." ;;
esac

publish=false
case $event in
    schedule | workflow_dispatch) publish=true ;;
esac

if [ -z "$ref" ]; then
    ref=$(curl -fsSL --max-time 60 "$PROXMOX_PACKAGES" | gzip -dc | newest_in)
    case $ref in
        [0-9]*.*-*) ;;
        *) die "Proxmox's repository names no pve-qemu-kvm." ;;
    esac
fi

version=
case $ref in
    [0-9]*.*-*) version=$ref$suffix ;;
esac

build=true
if [ "$publish" = true ] && [ -n "$version" ] && published | grep -qxF "$version"; then
    case $event in
        schedule)
            echo "$version is published already, so there's nothing to build tonight."
            build=false
            publish=false
            ;;
        *) die "$version is published already, and what's published never changes. Run it again with another suffix, such as +gpupv2." ;;
    esac
fi

out=${GITHUB_OUTPUT:-/dev/stdout}
{
    echo "ref=$ref"
    echo "version=$version"
    echo "build=$build"
    echo "publish=$publish"
} >>"$out"
