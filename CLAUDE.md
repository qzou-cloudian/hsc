# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`hsc` is a high-performance S3 CLI written in Rust. It supports AWS S3 and S3-compatible storage (MinIO, Cloudian, etc.) with optional RDMA-accelerated transfers. The binary exposes 17 subcommands for bucket/object operations, metadata inspection, sync, diffing, and performance benchmarking.

## Commands

### Build

```bash
cargo build                          # Debug build
cargo build --release                # Release build (includes the rdma feature)
cargo build --no-default-features     # Build without RDMA (rdma is a default feature)
```

### Lint & Format

```bash
cargo clippy -- -D warnings          # Lint (CI runs with warnings-as-errors)
cargo fmt --check                    # Check formatting
cargo fmt                            # Auto-format
```

### Tests

The project has no `cargo test` unit tests. Testing is done via integration shell scripts that require a live S3 endpoint:

```bash
cd tests
./test_s3.sh                    # Full integration suite
./test_multipart.sh             # Multipart upload tests
./test_stat_comprehensive.sh    # Stat command tests
./test_diff.sh                  # Diff command tests
./test_cmp.sh                   # Compare command tests
./quick_test.sh                 # Smoke test
```

### Docker-based Cross-Platform Builds

```bash
make docker-images    # One-time: build Ubuntu 24.04 and Rocky 8 builder images
make                  # Build binaries for both distros
make ubuntu           # Ubuntu 24.04 only
make rocky            # Rocky Linux 8 only
make packages         # Build RPM and DEB packages as well
```

Output goes to `dist/ubuntu-24.04/` and `dist/rocky-8/`. The Makefile uses named Docker volumes (`hsc-target-ubuntu-24.04`, `hsc-target-rocky-8`) for incremental builds. Every build produces `hsc` (rdma feature) plus `libs3rdmaclient.so` built with the s3-rdma-client `cuobj` feature; the builder images install the latest NVIDIA cuObject client SDK (`libcuobjclient-dev`, plus matching cuFile/cudart headers and ibverbs) from NVIDIA's CUDA repo, so rerun `make docker-images` to pick up a newer SDK. Override source paths with `S3RDMA_SRC=../s3-rdma CUOBJ_SRC=../cuobj`.

## Architecture

### Entry Point and CLI (`src/main.rs`)

Uses `clap` derive macros to define all global options and subcommands. After parsing, it builds an `aws_sdk_s3::Client` via `src/s3_client.rs` and dispatches to the appropriate command module. Global options include `--endpoint-url`, `--profile`, `--region`, `--debug`, `--no-verify-ssl`, `--no-sign-request`, `--resolve` (curl-style DNS override), and `--rdma [PROVIDER]`.

### S3 Client and Interceptors (`src/s3_client.rs`, `src/rdma/interceptor.rs`, `src/redirect_interceptor.rs`, `src/debug_interceptor.rs`)

The AWS SDK is configured with a chain of Smithy interceptors that run on each request/response:
- `DebugInterceptor` — logs HTTP method, URI, status (`--debug`)
- `AcceptAnyServerCert` — disables TLS verification (`--no-verify-ssl`)
- `NoSignRequestInterceptor` — strips auth headers (`--no-sign-request`)
- `CustomHeadersInterceptor` — injects `-H` headers; skips headers for non-final multipart parts
- `RedirectInterceptor` — handles 3xx responses
- `RdmaInterceptor` (feature-gated) — replaces the HTTP data path with RDMA transfers

When adding new global behaviors (auth, logging, header mutation), implement a new Smithy interceptor rather than modifying individual commands.

### Commands (`src/commands/`)

Each subcommand is a separate file. The largest/most complex ones:
- `cp.rs` — multipart upload/download, checksum validation (CRC32, CRC32C, SHA1, SHA256), SSE (AES256, KMS, customer key), RDMA path, progress reporting
- `sync.rs` — change detection (size + ETag), delete-after-sync, filtering
- `stat.rs` — comprehensive metadata display for objects and buckets
- `perf_object.rs` / `test_object.rs` — benchmarking and correctness verification with byte-range validation

### Path Handling (`src/path_utils.rs`)

`parse_path()` classifies a string as `Local`, `S3Bucket`, `S3Prefix`, or `S3Object`. All commands use this to decide whether source/destination is local filesystem or S3.

### Filtering (`src/filters.rs`)

`FileFilter` implements glob-based `--include` / `--exclude` patterns used by `cp`, `sync`, and `diff` for recursive operations.

### RDMA Feature (`src/rdma/`)

Compiled via the `rdma` feature, which is on by default. The `RdmaInterceptor` hooks into the Smithy HTTP layer to route data through RDMA (GPU-direct or CPU-direct) instead of the standard TCP stack. Requires the public `s3-rdma` crate as a path dependency at `../s3-rdma` (which itself needs `../cuobj` for its plugin). hsc loads the cuObject backend at runtime from `libs3rdmaclient.so`; only that plugin links the cuObject SDK.

## Important Conventions

- **No unit tests in-repo** — all verification is via shell integration tests against a real S3 endpoint.
- **RDMA is always built** — s3-rdma and cuobj are public repos; CI clones them next to hsc, builds hsc with the default `rdma` feature, and builds the cuobj plugin in the Docker builder images. No hardware tests run in CI.
- **Smithy interceptors for cross-cutting concerns** — new HTTP-level behaviors should be interceptors, not per-command logic.
- **Async throughout** — all commands are `async fn` using `tokio`. Multipart operations use `tokio::task::spawn` for concurrent part transfers.
- **Checksum propagation** — `cp` computes checksums client-side and passes them in `ChecksumAlgorithm` SDK fields; the server verifies. Be careful not to break this when touching upload paths.
- **`--resolve` DNS override** — implemented by building a custom `hyper` connector with a static DNS map; relevant code is in `s3_client.rs`.
