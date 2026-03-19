# =============================================================================
# hsc build system
#
# Builds the hsc binary inside Docker containers for:
#   Ubuntu 24.04  →  dist/ubuntu-24.04/
#   Rocky Linux 8 →  dist/rocky-8/
#
# Usage:
#   make docker-images          # build Docker builder images (once)
#   make                        # build for both platforms
#   make ubuntu                 # Ubuntu 24.04 only
#   make rocky                  # Rocky Linux 8 only
#   make FEATURES=rdma ubuntu   # without cuObject, RDMA mock only
#   make FEATURES= ubuntu       # default (no RDMA)
#   make clean                  # remove dist/
# =============================================================================

# ── Configurable variables ────────────────────────────────────────────────────

# Cargo features to enable (cuobj | rdma | <empty>)
FEATURES   ?= cuobj

# Cargo features for package builds (no RDMA/CUDA dependency by default)
PACKAGE_FEATURES ?=

# CUDA / cuObject SDK root on the host (must contain include/cuobjclient.h)
CUDA_DIR   ?= /usr/local/cuda-13.2

# Path to the cuobj Rust source tree (needed for the cuobj crate path dep)
CUOBJ_SRC  ?= $(abspath ../cuobj)

# Path to the s3-rdma Rust source tree (needed for the s3-rdma crate path dep)
S3RDMA_SRC ?= $(abspath ../s3-rdma)

# ── Internal variables ────────────────────────────────────────────────────────

HSC_DIR        := $(abspath .)
DIST_DIR       := $(HSC_DIR)/dist
CARGO_REGISTRY := $(HOME)/.cargo/registry
CARGO_GIT      := $(HOME)/.cargo/git

UBUNTU_IMAGE   := hsc-builder:ubuntu-24.04
ROCKY_IMAGE    := hsc-builder:rocky-8

ROCKY_RPM_IMAGE  := hsc-pkg-builder:rocky-8
UBUNTU_DEB_IMAGE := hsc-pkg-builder:ubuntu-24.04

# Per-platform Cargo target dirs (named Docker volumes — avoids rebuilding
# from scratch on every run and keeps host target/ dir uncontaminated).
UBUNTU_VOL     := hsc-target-ubuntu-24.04
ROCKY_VOL      := hsc-target-rocky-8

ROCKY_RPM_VOL  := hsc-target-rocky-rpm
UBUNTU_DEB_VOL := hsc-target-ubuntu-deb

# Build command run inside each container
define BUILD_CMD
set -e; \
cd /build/hsc; \
CUOBJ_ROOT_DIR=/usr/local/cuda \
cargo build --release $(if $(FEATURES),--features $(FEATURES)) \
    --target-dir /cargo/target; \
cp /cargo/target/release/hsc /out/; \
if echo "$(FEATURES)" | grep -q cuobj; then \
    cargo build --release \
        --manifest-path ../s3-rdma/providers/cuobj/Cargo.toml \
        --target-dir /cargo/target-cuobj; \
    cp /cargo/target-cuobj/release/libs3_rdma_cuobj.so /out/; \
fi
endef

# Common docker run flags
define DOCKER_RUN_FLAGS
--rm \
-v "$(HSC_DIR):/build/hsc:ro" \
-v "$(CUOBJ_SRC):/build/cuobj:ro" \
-v "$(S3RDMA_SRC):/build/s3-rdma:ro" \
-v "$(CUDA_DIR):/usr/local/cuda:ro" \
-v "$(CARGO_REGISTRY):/root/.cargo/registry" \
-v "$(CARGO_GIT):/root/.cargo/git"
endef

# Docker run flags for package builds (no CUDA/RDMA volumes required)
define PKG_DOCKER_RUN_FLAGS
--rm \
-v "$(HSC_DIR):/build/hsc:ro" \
-v "$(CARGO_REGISTRY):/root/.cargo/registry" \
-v "$(CARGO_GIT):/root/.cargo/git"
endef

# ── Top-level targets ─────────────────────────────────────────────────────────

.PHONY: all ubuntu rocky docker-images packages rpm deb docker-package-images clean help

all: ubuntu rocky

ubuntu: $(DIST_DIR)/ubuntu-24.04
	docker run $(DOCKER_RUN_FLAGS) \
	    -v "$(UBUNTU_VOL):/cargo/target" \
	    -v "$(DIST_DIR)/ubuntu-24.04:/out" \
	    $(UBUNTU_IMAGE) bash -c '$(BUILD_CMD)'

rocky: $(DIST_DIR)/rocky-8
	docker run $(DOCKER_RUN_FLAGS) \
	    -v "$(ROCKY_VOL):/cargo/target" \
	    -v "$(DIST_DIR)/rocky-8:/out" \
	    $(ROCKY_IMAGE) bash -c '$(BUILD_CMD)'

# ── Docker builder images ─────────────────────────────────────────────────────

docker-images:
	docker build --network=host -t $(UBUNTU_IMAGE) -f docker/Dockerfile.ubuntu docker/
	docker build --network=host -t $(ROCKY_IMAGE)  -f docker/Dockerfile.rocky  docker/

# ── Packaging ─────────────────────────────────────────────────────────────────
# Builds distribution packages inside Docker containers.
# PACKAGE_FEATURES defaults to empty (no RDMA/CUDA required).
# Override: make PACKAGE_FEATURES=rdma rpm

packages: rpm deb

rpm: $(DIST_DIR)/rocky-8
	docker run $(PKG_DOCKER_RUN_FLAGS) \
	    -v "$(ROCKY_RPM_VOL):/cargo/target" \
	    -v "$(DIST_DIR)/rocky-8:/out" \
	    -e FEATURES="$(PACKAGE_FEATURES)" \
	    $(ROCKY_RPM_IMAGE) bash /build/hsc/docker/build-rpm.sh

deb: $(DIST_DIR)/ubuntu-24.04
	docker run $(PKG_DOCKER_RUN_FLAGS) \
	    -v "$(UBUNTU_DEB_VOL):/cargo/target" \
	    -v "$(DIST_DIR)/ubuntu-24.04:/out" \
	    -e FEATURES="$(PACKAGE_FEATURES)" \
	    $(UBUNTU_DEB_IMAGE) bash /build/hsc/docker/build-deb.sh

docker-package-images:
	docker build --network=host -t $(ROCKY_RPM_IMAGE)  -f docker/Dockerfile.rocky-rpm  docker/
	docker build --network=host -t $(UBUNTU_DEB_IMAGE) -f docker/Dockerfile.ubuntu-deb docker/

# ── Helpers ───────────────────────────────────────────────────────────────────

$(DIST_DIR)/ubuntu-24.04 $(DIST_DIR)/rocky-8:
	mkdir -p $@

clean:
	rm -rf $(DIST_DIR)

# Remove Docker volumes (clears the incremental Cargo build cache)
clean-volumes:
	-docker volume rm $(UBUNTU_VOL) $(ROCKY_VOL) $(UBUNTU_DEB_VOL) $(ROCKY_RPM_VOL)

help:
	@echo ""
	@echo "Targets:"
	@echo "  all                  Build binaries for Ubuntu 24.04 and Rocky Linux 8 (default)"
	@echo "  ubuntu               Build binary for Ubuntu 24.04 only"
	@echo "  rocky                Build binary for Rocky Linux 8 only"
	@echo "  packages             Build RPM and DEB packages"
	@echo "  rpm                  Build RPM package for Rocky Linux 8"
	@echo "  deb                  Build DEB package for Ubuntu 24.04"
	@echo "  docker-images        Build binary builder images (run once)"
	@echo "  docker-package-images Build package builder images (run once)"
	@echo "  clean                Remove dist/"
	@echo "  clean-volumes        Remove Docker build-cache volumes (forces full Cargo rebuild)"
	@echo ""
	@echo "Variables (override on command line):"
	@echo "  FEATURES         Cargo features for binary builds  [default: cuobj]"
	@echo "                   cuobj  — RDMA via NVIDIA cuObject SDK (requires CUDA_DIR)"
	@echo "                   rdma   — RDMA mock only (no CUDA needed)"
	@echo "                   (empty)— no RDMA support"
	@echo "  PACKAGE_FEATURES Cargo features for package builds [default: (empty)]"
	@echo "  CUDA_DIR         CUDA/cuObject SDK root  [default: /usr/local/cuda-13.2]"
	@echo "  CUOBJ_SRC        cuobj Rust source tree  [default: ../cuobj]"
	@echo "  S3RDMA_SRC       s3-rdma Rust source tree  [default: ../s3-rdma]"
	@echo ""
	@echo "Output:"
	@echo "  dist/ubuntu-24.04/hsc"
	@echo "  dist/ubuntu-24.04/hsc_<version>_amd64.deb"
	@echo "  dist/ubuntu-24.04/libs3_rdma_cuobj.so  (when FEATURES=cuobj)"
	@echo "  dist/rocky-8/hsc"
	@echo "  dist/rocky-8/hsc-<version>-1.el8.x86_64.rpm"
	@echo "  dist/rocky-8/libs3_rdma_cuobj.so        (when FEATURES=cuobj)"
	@echo ""
