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

# Cargo features to enable (cuobject | rdma | <empty>)
FEATURES   ?= cuobject

# CUDA / cuObject SDK root on the host (must contain include/cuobjclient.h)
CUDA_DIR   ?= /usr/local/cuda-13.2

# Path to the cuobject Rust source tree (needed for the cuobject crate path dep)
CUOBJ_SRC  ?= $(abspath ../cuobject)

# ── Internal variables ────────────────────────────────────────────────────────

HSC_DIR        := $(abspath .)
DIST_DIR       := $(HSC_DIR)/dist
CARGO_REGISTRY := $(HOME)/.cargo/registry
CARGO_GIT      := $(HOME)/.cargo/git

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
CUOBJECT_ROOT_DIR=/usr/local/cuda \
cargo build --release $(if $(FEATURES),--features $(FEATURES)) \
    --target-dir /cargo/target; \
cp /cargo/target/release/hsc /out/; \
if echo "$(FEATURES)" | grep -q cuobject; then \
    cargo build --release \
        --manifest-path crates/hsc-rdma-cuobj/Cargo.toml \
        --target-dir /cargo/target-cuobj; \
    cp /cargo/target-cuobj/release/libhsc_rdma_cuobj.so /out/; \
fi
endef

# Common docker run flags
define DOCKER_RUN_FLAGS
--rm \
-v "$(HSC_DIR):/build/hsc:ro" \
-v "$(CUOBJ_SRC):/build/cuobject:ro" \
-v "$(CUDA_DIR):/usr/local/cuda:ro" \
-v "$(CARGO_REGISTRY):/root/.cargo/registry" \
-v "$(CARGO_GIT):/root/.cargo/git"
endef

# ── Top-level targets ─────────────────────────────────────────────────────────

.PHONY: all ubuntu rocky docker-images clean help

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
	@echo "  all            Build for Ubuntu 24.04 and Rocky Linux 8 (default)"
	@echo "  ubuntu         Build for Ubuntu 24.04 only"
	@echo "  rocky          Build for Rocky Linux 8 only"
	@echo "  docker-images  Build Docker builder images (run once, or after toolchain update)"
	@echo "  clean          Remove dist/"
	@echo "  clean-volumes  Remove Docker build-cache volumes (forces full Cargo rebuild)"
	@echo ""
	@echo "Variables (override on command line):"
	@echo "  FEATURES   Cargo features  [default: cuobject]"
	@echo "             cuobject  — RDMA via NVIDIA cuObject SDK (requires CUDA_DIR)"
	@echo "             rdma      — RDMA mock only (no CUDA needed)"
	@echo "             (empty)   — no RDMA support"
	@echo "  CUDA_DIR   CUDA/cuObject SDK root  [default: /usr/local/cuda-13.2]"
	@echo "  CUOBJ_SRC  cuobject Rust source tree  [default: ../cuobject]"
	@echo ""
	@echo "Output:"
	@echo "  dist/ubuntu-24.04/hsc"
	@echo "  dist/ubuntu-24.04/libhsc_rdma_cuobj.so  (when FEATURES=cuobject)"
	@echo "  dist/rocky-8/hsc"
	@echo "  dist/rocky-8/libhsc_rdma_cuobj.so        (when FEATURES=cuobject)"
	@echo ""
