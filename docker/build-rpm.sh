#!/bin/bash
# Build hsc RPM package inside a Rocky Linux 8 container.
# Expected mounts:
#   /build/hsc         — hsc source tree (read-only)
#   /cargo/target      — Cargo target directory (persistent Docker volume)
#   /out               — output directory for the .rpm file
set -e

cd /build/hsc

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"//;s/"//')
echo "==> Building hsc v${VERSION} RPM for Rocky Linux 8"

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

# Strip debug symbols to reduce package size
strip "$BINARY"

# Stage the binary where rpmbuild can find it
cp "$BINARY" ~/rpmbuild/SOURCES/hsc

# Generate the spec file
cat > ~/rpmbuild/SPECS/hsc.spec << EOF
Name:       hsc
Version:    ${VERSION}
Release:    1%{?dist}
Summary:    High-performance S3 CLI tool
License:    MIT
BuildArch:  x86_64
URL:        https://github.com/qzou-cloudian/hsc

%description
hsc is a high-performance S3 CLI tool written in Rust.
Supports AWS S3 and S3-compatible storage services (MinIO, Cloudian, etc.)
with multipart upload, checksum validation, range reads, and recursive sync.

%prep
# Binary pre-built by Cargo — nothing to prepare.

%build
# Binary pre-built by Cargo — nothing to build.

%install
install -Dm755 %{_sourcedir}/hsc %{buildroot}%{_bindir}/hsc

%files
%{_bindir}/hsc

%changelog
* $(LC_TIME=C date '+%a %b %d %Y') Qingshan <qzou@cloudian.com> - ${VERSION}-1
- Release ${VERSION}
EOF

echo "==> Running rpmbuild..."
rpmbuild -bb ~/rpmbuild/SPECS/hsc.spec

RPM=$(find ~/rpmbuild/RPMS/ -name "hsc-*.rpm" | head -1)
cp "$RPM" /out/
echo "==> Package ready: /out/$(basename "$RPM")"
