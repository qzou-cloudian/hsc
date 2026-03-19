#!/bin/bash
# Build hsc DEB package inside an Ubuntu 24.04 container.
# Expected mounts:
#   /build/hsc         — hsc source tree (read-only)
#   /cargo/target      — Cargo target directory (persistent Docker volume)
#   /out               — output directory for the .deb file
set -e

cd /build/hsc

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"//;s/"//')
echo "==> Building hsc v${VERSION} DEB for Ubuntu 24.04"

# Copy source to a writable working directory so Cargo can update Cargo.lock.
cp -rp /build/hsc /tmp/hsc-src

# Create a stub s3-rdma crate at the path Cargo.toml expects (../s3-rdma/core).
# The real crate is optional (rdma/cuobj features) so a stub that declares the
# required feature names is enough for dependency resolution to succeed.
mkdir -p /tmp/s3-rdma/core/src
cat > /tmp/s3-rdma/core/Cargo.toml << 'STUB'
[package]
name = "s3-rdma"
version = "0.1.0"
edition = "2021"

[features]
cuobj = []
STUB
printf '' > /tmp/s3-rdma/core/src/lib.rs

# Build the release binary from the writable copy
echo "==> Compiling (FEATURES=${FEATURES:-<none>})..."
cd /tmp/hsc-src
cargo build --release \
    ${FEATURES:+--features "$FEATURES"} \
    --target-dir /cargo/target

BINARY=/cargo/target/release/hsc
PKG_DIR=/tmp/hsc_${VERSION}_amd64

# Build the Debian package tree
mkdir -p "${PKG_DIR}/usr/bin"
mkdir -p "${PKG_DIR}/DEBIAN"

install -Dm755 "$BINARY" "${PKG_DIR}/usr/bin/hsc"

# Strip debug symbols to reduce package size
strip "${PKG_DIR}/usr/bin/hsc"

# Compute installed size (kB, as required by Debian policy)
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
dpkg-deb --build "${PKG_DIR}" "/out/${DEB_FILE}"
echo "==> Package ready: /out/${DEB_FILE}"
