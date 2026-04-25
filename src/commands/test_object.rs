use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_sdk_s3::Client;
use serde::Serialize;

use super::cmp::compare_paths;
use super::cp::{upload_file, SseConfig};

#[cfg(feature = "rdma")]
use crate::rdma::RdmaProvider;
#[cfg(feature = "rdma")]
use std::sync::Arc;

// ── Storage policy ────────────────────────────────────────────────────────────

/// Storage policy used by the target bucket.
///
/// Controls which structural boundaries are probed:
/// * `Ec` — erasure-coded storage; tests EC 4+2 and EC 2+1 stripe offsets within
///   each chunk in addition to chunk and part boundaries.
/// * `Replica` — replicated storage; tests only chunk and multipart part boundaries.
///   EC stripe tests are omitted because replicated objects have no intra-chunk
///   stripe structure.
#[derive(Clone, Debug, PartialEq, clap::ValueEnum)]
pub enum StoragePolicy {
    /// Erasure-coded storage — tests chunk, part, and EC stripe boundaries (default)
    Ec,
    /// Replicated storage — tests chunk and part boundaries only
    Replica,
}

// ── XorShift64 PRNG — no external dependency needed ──────────────────────────

fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn generate_random_data(size: u64) -> Vec<u8> {
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xdeadbeefcafebabe)
        ^ (std::process::id() as u64 * 6364136223846793005);
    if state == 0 {
        state = 0xdeadbeefcafebabe;
    }
    let mut buf = Vec::with_capacity(size as usize);
    while buf.len() < size as usize {
        let word = xorshift64(&mut state);
        let needed = (size as usize - buf.len()).min(8);
        buf.extend_from_slice(&word.to_le_bytes()[..needed]);
    }
    buf
}

// ── Test case types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct TestCase {
    description: String,
    range: Option<String>,
}

#[derive(Debug, Serialize)]
struct TestResultJson {
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<String>,
    passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SummaryJson {
    bucket: String,
    key: String,
    file: String,
    file_size: u64,
    chunk_size: u64,
    part_size: u64,
    policy: String,
    passed: usize,
    failed: usize,
    tests: Vec<TestResultJson>,
}

// ── Range generation ──────────────────────────────────────────────────────────

/// A boundary to probe (an offset within the object where a structural edge lies).
///
/// `major` is true for chunk and part boundaries (which get wider straddles);
/// false for EC stripe boundaries (2B + 8B straddles are sufficient).
struct Boundary {
    label: String,
    offset: u64,
    major: bool,
}

/// Collect all structural boundaries for an object of `file_size` bytes given
/// `chunk_size` (server storage chunk) and `part_size` (multipart upload part).
///
/// When `policy` is `Replica`, EC stripe boundaries are omitted because replicated
/// objects have no intra-chunk stripe structure.
fn collect_boundaries(
    file_size: u64,
    chunk_size: u64,
    part_size: u64,
    policy: &StoragePolicy,
) -> Vec<Boundary> {
    let mut boundaries: Vec<Boundary> = Vec::new();

    let s42 = chunk_size / 4; // EC 4+2 stripe
    let s21 = chunk_size / 2; // EC 2+1 stripe

    // ── Multipart part boundaries (major) ────────────────────────────────────
    let mut k = 1u64;
    while k * part_size < file_size {
        boundaries.push(Boundary {
            label: format!("part boundary {}×part_size ({})", k, k * part_size),
            offset: k * part_size,
            major: true,
        });
        k += 1;
    }

    // ── Chunk and EC stripe boundaries ────────────────────────────────────────
    let mut chunk_idx = 0u64;
    loop {
        let chunk_start = chunk_idx * chunk_size;
        if chunk_start >= file_size {
            break;
        }

        // Chunk boundary (start of chunk > 0) — major
        if chunk_idx > 0 {
            boundaries.push(Boundary {
                label: format!("chunk boundary {}×C ({})", chunk_idx, chunk_start),
                offset: chunk_start,
                major: true,
            });
        }

        // EC stripe offsets within this chunk — not major; skipped for replica policy
        if *policy == StoragePolicy::Ec {
            for (stripe_offset, stripe_label) in [
                (
                    s42,
                    format!(
                        "EC4+2 stripe-1 in chunk {} ({})",
                        chunk_idx,
                        chunk_start + s42
                    ),
                ),
                (
                    s21,
                    format!(
                        "EC2+1  stripe   in chunk {} ({})",
                        chunk_idx,
                        chunk_start + s21
                    ),
                ),
                (
                    3 * s42,
                    format!(
                        "EC4+2 stripe-3 in chunk {} ({})",
                        chunk_idx,
                        chunk_start + 3 * s42
                    ),
                ),
            ] {
                let abs_offset = chunk_start + stripe_offset;
                if abs_offset < file_size {
                    boundaries.push(Boundary {
                        label: stripe_label,
                        offset: abs_offset,
                        major: false,
                    });
                }
            }
        }

        chunk_idx += 1;
    }

    boundaries
}

/// Build all test cases for an object of `file_size` bytes.
fn build_test_cases(
    file_size: u64,
    chunk_size: u64,
    part_size: u64,
    policy: &StoragePolicy,
) -> Vec<TestCase> {
    let mut cases: Vec<TestCase> = Vec::new();
    let mut seen_ranges: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut add = |description: &str, range: Option<String>| {
        let key = range.clone().unwrap_or_default();
        if seen_ranges.insert(key) {
            cases.push(TestCase {
                description: description.to_string(),
                range,
            });
        }
    };

    // ── Fixed edge cases ──────────────────────────────────────────────────────
    add("whole object", None);

    if file_size > 0 {
        add("first byte", Some("bytes=0-0".to_string()));
        add(
            "last byte",
            Some(format!("bytes={}-{}", file_size - 1, file_size - 1)),
        );
    }
    if file_size >= 4 {
        add(
            "last 4 bytes",
            Some(format!("bytes={}-{}", file_size - 4, file_size - 1)),
        );
    }

    // ── Boundary-driven cases ─────────────────────────────────────────────────
    // Every boundary gets a 2-byte straddle and 8-byte crossing.
    // Major boundaries (chunk / part) also get 1 KiB and 4 KiB straddles,
    // because most real-world server reassembly bugs appear at those seams.
    let boundaries = collect_boundaries(file_size, chunk_size, part_size, policy);

    for b in &boundaries {
        let off = b.offset;
        if off == 0 || off >= file_size {
            continue;
        }

        // 2-byte straddle: [off-1, off]
        add(
            &format!("{} — 2B straddle", b.label),
            Some(format!("bytes={}-{}", off - 1, off)),
        );

        // 8-byte crossing: [off-4, off+3] (clamped)
        let start = off.saturating_sub(4);
        let end = (off + 3).min(file_size - 1);
        if start < end {
            add(
                &format!("{} — 8B crossing", b.label),
                Some(format!("bytes={}-{}", start, end)),
            );
        }

        if b.major {
            // 1 KiB straddle: covers the user-reported failure case (2 KiB range
            // crossing a 4 MiB chunk boundary returned wrong bytes on a buggy server)
            let w = 512u64;
            let start = off.saturating_sub(w);
            let end = (off + w - 1).min(file_size - 1);
            if start < end {
                add(
                    &format!("{} — 1KiB straddle", b.label),
                    Some(format!("bytes={}-{}", start, end)),
                );
            }

            // 4 KiB straddle: tests larger cross-boundary reads
            let w = 2048u64;
            let start = off.saturating_sub(w);
            let end = (off + w - 1).min(file_size - 1);
            if start < end {
                add(
                    &format!("{} — 4KiB straddle", b.label),
                    Some(format!("bytes={}-{}", start, end)),
                );
            }
        }
    }

    // ── Chunk-level structural tests ──────────────────────────────────────────
    build_chunk_cases(file_size, chunk_size, policy, &mut add);

    // ── Extra multipart tests ─────────────────────────────────────────────────
    build_multipart_cases(file_size, part_size, &mut add);

    cases
}

/// Generate chunk-level test cases: complete chunk reads, adjacent 2-chunk reads,
/// cross-EC-stripe reads within a single chunk, and large-range reads.
///
/// Cross-EC reads are skipped when `policy` is `Replica`.
fn build_chunk_cases(
    file_size: u64,
    chunk_size: u64,
    policy: &StoragePolicy,
    add: &mut impl FnMut(&str, Option<String>),
) {
    if file_size == 0 {
        return;
    }

    let num_chunks = file_size.div_ceil(chunk_size);
    let s42 = chunk_size / 4;

    // ── Complete single-chunk reads ───────────────────────────────────────────
    // Verifies the server can serve exactly one chunk of data end-to-end.
    for k in 0..num_chunks {
        let start = k * chunk_size;
        let end = ((k + 1) * chunk_size - 1).min(file_size - 1);
        add(
            &format!("complete chunk {} ({}-{})", k, start, end),
            Some(format!("bytes={}-{}", start, end)),
        );
    }

    // ── Adjacent 2-chunk reads ────────────────────────────────────────────────
    // Verifies that multi-chunk assembly works for every adjacent pair.
    for k in 0..num_chunks.saturating_sub(1) {
        let start = k * chunk_size;
        let end = ((k + 2) * chunk_size - 1).min(file_size - 1);
        add(
            &format!("2-chunk span: chunks {}-{}", k, k + 1),
            Some(format!("bytes={}-{}", start, end)),
        );
    }

    // ── Cross-all-EC-stripes read within each chunk ───────────────────────────
    // A single range that crosses EC4+2 stripe-1, EC2+1 stripe, and EC4+2 stripe-3,
    // landing 4 bytes before the first stripe and 4 bytes after the last.
    // Skipped for replica storage (no intra-chunk stripe structure).
    if *policy == StoragePolicy::Ec {
        for k in 0..num_chunks {
            let base = k * chunk_size;
            let start = base + s42.saturating_sub(4);
            let end = (base + 3 * s42 + 3).min(file_size - 1);
            if start < end {
                add(
                    &format!("cross-all-EC within chunk {} (EC4+2_1 → EC4+2_3)", k),
                    Some(format!("bytes={}-{}", start, end)),
                );
            }
        }
    }

    // ── Large-range reads ─────────────────────────────────────────────────────
    // Span a significant fraction of the file; catches bugs that only appear
    // when reassembling many chunks/stripes in a single response.
    if file_size >= 4 {
        // First half
        add(
            "large range: first half",
            Some(format!("bytes=0-{}", file_size / 2 - 1)),
        );

        // Second half
        add(
            "large range: second half",
            Some(format!("bytes={}-{}", file_size / 2, file_size - 1)),
        );

        // First three-quarters (crosses the midpoint that the halves share)
        if file_size >= 8 {
            add(
                "large range: first 3/4",
                Some(format!("bytes=0-{}", 3 * file_size / 4 - 1)),
            );
        }
    }
}

/// Generate extra test cases that are specific to multipart-uploaded objects.
///
/// These probe scenarios where bugs most commonly appear in S3 server reassembly:
/// - Reads that start or end exactly at a part seam
/// - Reading exactly one complete part
/// - Ranges that span two or more complete parts
/// - The last (possibly partial) part
///
/// Note: 1 KiB and 4 KiB straddles across part seams are now handled generically
/// by the major-boundary logic in `build_test_cases`.
fn build_multipart_cases(
    file_size: u64,
    part_size: u64,
    add: &mut impl FnMut(&str, Option<String>),
) {
    if file_size <= part_size {
        // Single-part PUT — no multipart seams to probe
        return;
    }

    let num_parts = file_size.div_ceil(part_size);

    // ── Per-seam edge reads ───────────────────────────────────────────────────
    for k in 1..num_parts {
        let seam = k * part_size; // byte offset of the start of part k+1
        if seam >= file_size {
            break;
        }

        let part_label = format!("part seam {}×P ({})", k, seam);

        // Last 4 bytes of part k (read ending exactly at seam)
        if seam >= 4 {
            add(
                &format!("{} — last 4B of part {}", part_label, k),
                Some(format!("bytes={}-{}", seam - 4, seam - 1)),
            );
        }

        // First 4 bytes of part k+1 (read starting exactly at seam)
        if seam + 3 < file_size {
            add(
                &format!("{} — first 4B of part {}", part_label, k + 1),
                Some(format!("bytes={}-{}", seam, seam + 3)),
            );
        }
    }

    // ── Complete single-part reads ────────────────────────────────────────────
    // First complete part
    add(
        "complete part 1",
        Some(format!("bytes=0-{}", part_size - 1)),
    );

    // Second complete part (if it exists)
    if file_size > 2 * part_size {
        add(
            "complete part 2",
            Some(format!("bytes={}-{}", part_size, 2 * part_size - 1)),
        );
    }

    // Last part (always partial unless file_size is an exact multiple of part_size)
    let last_part_start = (num_parts - 1) * part_size;
    if last_part_start > 0 && last_part_start < file_size {
        add(
            &format!("last (partial) part {}", num_parts),
            Some(format!("bytes={}-{}", last_part_start, file_size - 1)),
        );
    }

    // ── Multi-part spanning reads ─────────────────────────────────────────────
    // First 2 complete parts
    if file_size >= 2 * part_size {
        add(
            "first 2 complete parts",
            Some(format!("bytes=0-{}", 2 * part_size - 1)),
        );
    }

    // Last 2 parts (the last complete part + the tail, or the last 2 complete parts)
    if num_parts >= 3 {
        let start = (num_parts - 2) * part_size;
        add(
            &format!("last 2 parts (parts {}-{})", num_parts - 1, num_parts),
            Some(format!("bytes={}-{}", start, file_size - 1)),
        );
    }

    // All parts except the last (tests reading up to but not including the final seam)
    if num_parts >= 3 {
        let end = (num_parts - 1) * part_size - 1;
        add(
            &format!("all parts except last ({} parts)", num_parts - 1),
            Some(format!("bytes=0-{}", end)),
        );
    }
}

// ── Temp file helper ──────────────────────────────────────────────────────────

fn unique_temp_path() -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("hsc-test-{}-{}.dat", ts, std::process::id()))
}

// ── Main entry point ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub async fn test_object(
    client: &Client,
    bucket: &str,
    key: Option<&str>,
    file: Option<&str>,
    bytes: Option<u64>,
    chunk_size: u64,
    part_size: u64,
    policy: &StoragePolicy,
    keep: bool,
    json: bool,
    #[cfg(feature = "rdma")] rdma: Option<Arc<dyn RdmaProvider>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. Prepare local file ─────────────────────────────────────────────────
    let (local_path, temp_file, file_size): (String, Option<PathBuf>, u64) = match (file, bytes) {
        (Some(f), None) => {
            let meta = tokio::fs::metadata(f)
                .await
                .map_err(|e| format!("Cannot access '{}': {}", f, e))?;
            (f.to_string(), None, meta.len())
        }
        (None, Some(b)) => {
            let tmp = unique_temp_path();
            let data = generate_random_data(b);
            tokio::fs::write(&tmp, &data)
                .await
                .map_err(|e| format!("Failed to write temp file: {}", e))?;
            let path_str = tmp.to_string_lossy().into_owned();
            (path_str, Some(tmp), b)
        }
        (Some(_), Some(_)) => {
            return Err("Cannot specify both --file and --bytes".into());
        }
        (None, None) => {
            return Err("Specify either --file <path> or --bytes <size>".into());
        }
    };

    // ── 2. Determine S3 key ───────────────────────────────────────────────────
    let s3_key = match key {
        Some(k) => k.to_string(),
        None => {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            format!("hsc-test-{}-{}", ts, std::process::id())
        }
    };
    let s3_uri = format!("s3://{}/{}", bucket, s3_key);

    if !json {
        let source_label = if temp_file.is_some() {
            " (generated)"
        } else {
            ""
        };
        println!("Testing object: {} ({} bytes)", s3_uri, file_size);
        println!("  Local file: {}{}", local_path, source_label);
        println!(
            "  Chunk size: {}  Part size: {}  Policy: {:?}",
            chunk_size, part_size, policy
        );
    }

    // ── 3. Upload ─────────────────────────────────────────────────────────────
    if !json {
        print!("  Uploading to {}... ", s3_uri);
        std::io::stdout().flush().ok();
    }

    let upload_result = upload_file(
        client,
        &local_path,
        bucket,
        &s3_key,
        None, // checksum_mode
        None, // checksum_algorithm
        &SseConfig {
            sse: None,
            sse_kms_key_id: None,
            sse_c: None,
            sse_c_key: None,
            sse_c_copy_source: None,
            sse_c_copy_source_key: None,
        },
        part_size, // multipart_threshold
        part_size, // multipart_chunksize
        #[cfg(feature = "rdma")]
        rdma.as_ref().map(Arc::clone),
    )
    .await;

    if let Err(e) = upload_result {
        // Clean up temp file on upload error
        if let Some(ref tmp) = temp_file {
            let _ = tokio::fs::remove_file(tmp).await;
        }
        if !json {
            println!("FAILED");
        }
        return Err(format!("Upload failed: {}", e).into());
    }

    if !json {
        println!("ok");
    }

    // ── 4. Build test cases ───────────────────────────────────────────────────
    let cases = build_test_cases(file_size, chunk_size, part_size, policy);

    // ── 5. Run tests ──────────────────────────────────────────────────────────
    let mut results: Vec<TestResultJson> = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;

    for case in &cases {
        let cmp_result = compare_paths(
            client,
            &local_path,
            &s3_uri,
            case.range.clone(),
            None,
            None,
            None,
            None,
            #[cfg(feature = "rdma")]
            rdma.as_ref().map(Arc::clone),
        )
        .await;

        let (ok, err_msg) = match cmp_result {
            Ok(report) => {
                if report.identical {
                    (true, None)
                } else {
                    let reason = report
                        .difference
                        .map(|d| d.kind)
                        .unwrap_or_else(|| "content differs".to_string());
                    (false, Some(reason))
                }
            }
            Err(e) => (false, Some(e.to_string())),
        };

        if ok {
            passed += 1;
        } else {
            failed += 1;
        }

        if !json {
            let tag = if ok { "PASS" } else { "FAIL" };
            let err_suffix = err_msg
                .as_deref()
                .map(|e| format!(" — {}", e))
                .unwrap_or_default();
            if let Some(range) = &case.range {
                println!(
                    "  [{}] {}  ({}){}",
                    tag, range, case.description, err_suffix
                );
            } else {
                println!("  [{}] whole object{}", tag, err_suffix);
            }
        }

        results.push(TestResultJson {
            description: case.description.clone(),
            range: case.range.clone(),
            passed: ok,
            error: err_msg,
        });
    }

    // ── 6. Delete S3 object (unless --keep) ───────────────────────────────────
    if !keep {
        let del_result = client
            .delete_object()
            .bucket(bucket)
            .key(&s3_key)
            .send()
            .await;
        if let Err(e) = del_result {
            if !json {
                eprintln!("Warning: failed to delete {}: {}", s3_uri, e);
            }
        }
    }

    // Clean up local temp file
    if let Some(ref tmp) = temp_file {
        let _ = tokio::fs::remove_file(tmp).await;
    }

    // ── 7. Report ─────────────────────────────────────────────────────────────
    if json {
        let summary = SummaryJson {
            bucket: bucket.to_string(),
            key: s3_key.clone(),
            file: local_path.clone(),
            file_size,
            chunk_size,
            part_size,
            policy: format!("{:?}", policy).to_ascii_lowercase(),
            passed,
            failed,
            tests: results,
        };
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!();
        println!(
            "Result: {} passed, {} failed  ({})",
            passed,
            failed,
            if failed == 0 { "OK" } else { "FAILED" }
        );
    }

    if failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}
