#!/bin/bash
# Package pre-built hsc artifacts as an RPM for Rocky Linux 8.
#
# Expected bind-mounts:
#   /pkg  — dist/rocky-8/ directory containing hsc (and optionally libs3_rdma_cuobj.so)
#
# Required environment:
#   VERSION — package version string (e.g. "0.2.0")
set -e

VERSION=${VERSION:?VERSION env var must be set}
BINARY=/pkg/hsc
SO=/pkg/libs3_rdma_cuobj.so

echo "==> Packaging hsc v${VERSION} as RPM (Rocky Linux 8)"

[ -f "$BINARY" ] || { echo "ERROR: $BINARY not found — run 'make rocky' first."; exit 1; }

# Stage binaries for rpmbuild
cp "$BINARY" ~/rpmbuild/SOURCES/hsc

SO_INSTALL=""
SO_FILES=""
if [ -f "$SO" ]; then
    cp "$SO" ~/rpmbuild/SOURCES/libs3_rdma_cuobj.so
    SO_INSTALL="install -Dm755 %{_sourcedir}/libs3_rdma_cuobj.so %{buildroot}%{_libdir}/libs3_rdma_cuobj.so"
    SO_FILES="%{_libdir}/libs3_rdma_cuobj.so"
    echo "==> Including libs3_rdma_cuobj.so"
fi

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
%build

%install
install -Dm755 %{_sourcedir}/hsc %{buildroot}%{_bindir}/hsc
${SO_INSTALL}

%files
%{_bindir}/hsc
${SO_FILES}

%changelog
* $(LC_TIME=C date '+%a %b %d %Y') Qingshan <qzou@cloudian.com> - ${VERSION}-1
- Release ${VERSION}
EOF

echo "==> Running rpmbuild..."
rpmbuild -bb ~/rpmbuild/SPECS/hsc.spec

RPM=$(find ~/rpmbuild/RPMS/ -name "hsc-*.rpm" | head -1)
cp "$RPM" /pkg/
echo "==> Package ready: /pkg/$(basename "$RPM")"
