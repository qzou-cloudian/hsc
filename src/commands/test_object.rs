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
    passed: usize,
    failed: usize,
    tests: Vec<TestResultJson>,
}

// ── Range generation ──────────────────────────────────────────────────────────

/// A boundary to probe (an offset within the object where a structural edge lies).
struct Boundary {
    label: String,
    offset: u64,
}

/// Collect all structural boundaries for an object of `file_size` bytes given
/// `chunk_size` (server storage chunk) and `part_size` (multipart upload part).
fn collect_boundaries(file_size: u64, chunk_size: u64, part_size: u64) -> Vec<Boundary> {
    let mut boundaries: Vec<Boundary> = Vec::new();

    let s42 = chunk_size / 4; // EC 4+2 stripe
    let s21 = chunk_size / 2; // EC 2+1 stripe

    // ── Multipart part boundaries ─────────────────────────────────────────────
    let mut k = 1u64;
    while k * part_size < file_size {
        boundaries.push(Boundary {
            label: format!("part boundary {}×part_size ({})", k, k * part_size),
            offset: k * part_size,
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

        // Chunk boundary (start of chunk > 0)
        if chunk_idx > 0 {
            boundaries.push(Boundary {
                label: format!("chunk boundary {}×C ({})", chunk_idx, chunk_start),
                offset: chunk_start,
            });
        }

        // EC stripe offsets within this chunk
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
                });
            }
        }

        chunk_idx += 1;
    }

    boundaries
}

/// Build all test cases for an object of `file_size` bytes.
fn build_test_cases(file_size: u64, chunk_size: u64, part_size: u64) -> Vec<TestCase> {
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
    let boundaries = collect_boundaries(file_size, chunk_size, part_size);

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

        // 8-byte crossing: [off-4, off+3] (clamped to file bounds)
        let start = off.saturating_sub(4);
        let end = (off + 3).min(file_size - 1);
        if start < end {
            add(
                &format!("{} — 8B crossing", b.label),
                Some(format!("bytes={}-{}", start, end)),
            );
        }
    }

    cases
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
        println!("  Chunk size: {}  Part size: {}", chunk_size, part_size);
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
    let cases = build_test_cases(file_size, chunk_size, part_size);

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
