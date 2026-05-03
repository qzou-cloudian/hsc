#!/bin/bash
# Package pre-built hsc artifacts as a DEB for Ubuntu 24.04.
#
# Expected bind-mounts:
#   /pkg  — dist/ubuntu-24.04/ directory containing hsc (and optionally libs3rdmaclient.so)
#
# Required environment:
#   VERSION — package version string (e.g. "0.2.0")
set -e

VERSION=${VERSION:?VERSION env var must be set}
BINARY=/pkg/hsc
SO=/pkg/libs3rdmaclient.so

echo "==> Packaging hsc v${VERSION} as DEB (Ubuntu 24.04)"

[ -f "$BINARY" ] || { echo "ERROR: $BINARY not found — run 'make ubuntu' first."; exit 1; }

PKG_DIR=/tmp/hsc_${VERSION}_amd64
mkdir -p "${PKG_DIR}/usr/bin"
mkdir -p "${PKG_DIR}/DEBIAN"

install -m755 "$BINARY" "${PKG_DIR}/usr/bin/hsc"

if [ -f "$SO" ]; then
    mkdir -p "${PKG_DIR}/usr/lib"
    install -m755 "$SO" "${PKG_DIR}/usr/lib/libs3rdmaclient.so"
    echo "==> Including libs3rdmaclient.so"
fi

INSTALLED_SIZE=$(du -sk "${PKG_DIR}/usr" | cut -f1)

cat > "${PKG_DIR}/DEBIAN/control" << EOF
Package: hsc
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: amd64
Installed-Size: ${INSTALLED_SIZE}
Maintainer: Qingshan <qzou@cloudian.com>
Homepage: https://github.com/qzou-cloudian/hsc
Description: High-performance S3 CLI tool
 hsc is a high-performance S3 CLI tool written in Rust.
 Supports AWS S3 and S3-compatible storage services (MinIO, Cloudian, etc.)
 with multipart upload, checksum validation, range reads, and recursive sync.
EOF

DEB_FILE="hsc_${VERSION}_amd64.deb"
dpkg-deb --build "${PKG_DIR}" "/pkg/${DEB_FILE}"
echo "==> Package ready: /pkg/${DEB_FILE}"
