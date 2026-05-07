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

# Cargo features to enable (rdma | <empty>)
FEATURES   ?= rdma

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

# Version extracted from Cargo.toml (used by rpm / deb targets)
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*= *"//;s/"//')

UBUNTU_IMAGE   := hsc-builder:ubuntu-24.04
ROCKY_IMAGE    := hsc-builder:rocky-8

# Per-platform Cargo target dirs (named Docker volumes — avoids rebuilding
# from scratch on every run and keeps host target/ dir uncontaminated).
UBUNTU_VOL     := hsc-target-ubuntu-24.04
ROCKY_VOL      := hsc-target-rocky-8

# Build command run inside each container
define BUILD_CMD
set -e; \
cd /build/hsc; \
CUOBJ_ROOT_DIR=/usr/local/cuda \
cargo build --release $(if $(FEATURES),--features $(FEATURES)) \
    --target-dir /cargo/target; \
cp /cargo/target/release/hsc /out/; \
if echo "$(FEATURES)" | grep -q rdma; then \
    cargo build --release \
        --manifest-path ../s3-rdma/Cargo.toml \
        -p s3-rdma-client \
        --features cuobj \
        --target-dir /cargo/target-cuobj; \
    cp /cargo/target-cuobj/release/libs3rdmaclient.so /out/; \
fi
endef

# Common docker run flags
define DOCKER_RUN_FLAGS
--rm \
-v "$(HSC_DIR):/build/hsc:ro" \
-v "$(CUOBJ_SRC):/build/cuobj:ro" \
-v "$(S3RDMA_SRC):/build/s3-rdma" \
-v "$(CUDA_DIR):/usr/local/cuda:ro" \
-v "$(CARGO_REGISTRY):/root/.cargo/registry" \
-v "$(CARGO_GIT):/root/.cargo/git"
endef

# ── Top-level targets ─────────────────────────────────────────────────────────

.PHONY: all ubuntu rocky docker-images packages rpm deb clean help

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
# Builds distribution packages from the pre-compiled binaries produced by
# 'make rocky' / 'make ubuntu'.  Both hsc and libs3rdmaclient.so (when
# present) are included in the package.

packages: rpm deb

rpm: rocky
	docker run --rm \
	    -v "$(DIST_DIR)/rocky-8:/pkg" \
	    -v "$(HSC_DIR)/docker:/scripts:ro" \
	    -e VERSION=$(VERSION) \
	    $(ROCKY_IMAGE) bash /scripts/build-rpm.sh

deb: ubuntu
	docker run --rm \
	    -v "$(DIST_DIR)/ubuntu-24.04:/pkg" \
	    -v "$(HSC_DIR)/docker:/scripts:ro" \
	    -e VERSION=$(VERSION) \
	    $(UBUNTU_IMAGE) bash /scripts/build-deb.sh

# ── Helpers ───────────────────────────────────────────────────────────────────

$(DIST_DIR)/ubuntu-24.04 $(DIST_DIR)/rocky-8:
	mkdir -p $@

clean:
	rm -rf $(DIST_DIR)

# Remove Docker volumes (clears the incremental Cargo build cache)
clean-volumes:
	-docker volume rm $(UBUNTU_VOL) $(ROCKY_VOL)

help:
	@echo ""
	@echo "Targets:"
	@echo "  all                  Build binaries for Ubuntu 24.04 and Rocky Linux 8 (default)"
	@echo "  ubuntu               Build binary for Ubuntu 24.04 only"
	@echo "  rocky                Build binary for Rocky Linux 8 only"
	@echo "  packages             Build RPM and DEB packages (runs rocky + ubuntu first)"
	@echo "  rpm                  Build RPM for Rocky Linux 8   (runs 'make rocky' first)"
	@echo "  deb                  Build DEB for Ubuntu 24.04    (runs 'make ubuntu' first)"
	@echo "  docker-images        Build builder images (run once, or after toolchain update)"
	@echo "  clean                Remove dist/"
	@echo "  clean-volumes        Remove Docker build-cache volumes (forces full Cargo rebuild)"
	@echo ""
	@echo "Variables (override on command line):"
	@echo "  FEATURES   Cargo features  [default: rdma]"
	@echo "             rdma — RDMA via cuobj SDK (requires CUDA_DIR)"
	@echo "             (empty)— no RDMA support"
	@echo "  CUDA_DIR   CUDA/cuObject SDK root  [default: /usr/local/cuda-13.2]"
	@echo "  CUOBJ_SRC  cuobj Rust source tree  [default: ../cuobj]"
	@echo "  S3RDMA_SRC s3-rdma Rust source tree  [default: ../s3-rdma]"
	@echo ""
	@echo "Output:"
	@echo "  dist/ubuntu-24.04/hsc"
	@echo "  dist/ubuntu-24.04/libs3rdmaclient.so  (when FEATURES=rdma)"
	@echo "  dist/ubuntu-24.04/hsc_<version>_amd64.deb"
	@echo "  dist/rocky-8/hsc"
	@echo "  dist/rocky-8/libs3rdmaclient.so        (when FEATURES=rdma)"
	@echo "  dist/rocky-8/hsc-<version>-1.el8.x86_64.rpm"
	@echo ""
