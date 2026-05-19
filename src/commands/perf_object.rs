use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_sdk_s3::Client;
use serde::Serialize;
use tokio::sync::Semaphore;

use super::cp::{upload_file, UploadOptions};
use super::listing::list_s3_keys_limited;
use super::transfer::{MultipartConfig, SseConfig};

// ── Random data generator ─────────────────────────────────────────────────────

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

// ── Latency stats ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct LatencyStats {
    pub min: f64,
    pub avg: f64,
    pub max: f64,
    pub p99: f64,
}

fn compute_latency_stats(mut samples: Vec<f64>) -> LatencyStats {
    if samples.is_empty() {
        return LatencyStats {
            min: 0.0,
            avg: 0.0,
            max: 0.0,
            p99: 0.0,
        };
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = samples.len();
    let min = samples[0];
    let max = samples[n - 1];
    let avg = samples.iter().sum::<f64>() / n as f64;
    let p99_idx = ((n * 99).saturating_sub(1)) / 100;
    let p99 = samples[p99_idx.min(n - 1)];
    LatencyStats { min, avg, max, p99 }
}

// ── JSON output structs ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct BenchmarkResult {
    operation: String,
    bucket: String,
    prefix: String,
    workers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_size_bytes: Option<u64>,
    operations_completed: u64,
    elapsed_secs: f64,
    ops_per_sec: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    mb_per_sec: Option<f64>,
    latency_ms: LatencyStats,
}

impl BenchmarkResult {
    fn new(
        operation: &str,
        bucket: String,
        prefix: String,
        workers: usize,
        object_size_bytes: Option<u64>,
        metrics: BenchmarkMetrics,
    ) -> Self {
        Self {
            operation: operation.to_string(),
            bucket,
            prefix,
            workers,
            object_size_bytes,
            operations_completed: metrics.ops,
            elapsed_secs: metrics.elapsed_secs,
            ops_per_sec: metrics.ops_per_sec,
            mb_per_sec: metrics.mb_per_sec,
            latency_ms: metrics.latency,
        }
    }
}

struct BenchmarkMetrics {
    ops: u64,
    elapsed_secs: f64,
    ops_per_sec: f64,
    mb_per_sec: Option<f64>,
    latency: LatencyStats,
}

fn benchmark_mode_label(objects: Option<u64>, duration_secs: Option<u64>) -> String {
    objects
        .map(|n| format!("objects={}", n))
        .or_else(|| duration_secs.map(|s| format!("duration={}s", s)))
        .unwrap_or_default()
}

fn benchmark_deadline(duration_secs: Option<u64>) -> Option<Instant> {
    duration_secs.map(|s| Instant::now() + std::time::Duration::from_secs(s))
}

fn calculate_ops_per_sec(ops: u64, elapsed_secs: f64) -> f64 {
    if elapsed_secs > 0.0 {
        ops as f64 / elapsed_secs
    } else {
        0.0
    }
}

fn benchmark_metrics(
    ops: u64,
    elapsed_secs: f64,
    latencies: Vec<f64>,
    mb_per_sec: Option<f64>,
) -> BenchmarkMetrics {
    BenchmarkMetrics {
        ops,
        elapsed_secs,
        ops_per_sec: calculate_ops_per_sec(ops, elapsed_secs),
        mb_per_sec,
        latency: compute_latency_stats(latencies),
    }
}

fn print_benchmark_results(metrics: &BenchmarkMetrics) {
    eprintln!();
    println!("\nResults:");
    println!("  Operations:  {}", metrics.ops);
    println!("  Elapsed:     {:.1}s", metrics.elapsed_secs);
    if let Some(mb) = metrics.mb_per_sec {
        println!(
            "  Throughput:  {:.2} ops/s   {:.1} MB/s",
            metrics.ops_per_sec, mb
        );
    } else {
        println!("  Throughput:  {:.2} ops/s", metrics.ops_per_sec);
    }
    println!(
        "  Latency:     min={:.0}ms  avg={:.0}ms  max={:.0}ms  p99={:.0}ms",
        metrics.latency.min, metrics.latency.avg, metrics.latency.max, metrics.latency.p99
    );
}

// ── Progress line printer ─────────────────────────────────────────────────────

fn print_progress(completed: u64, total: Option<u64>) {
    if let Some(t) = total {
        eprint!("\r  {}/{}", completed, t);
    } else {
        eprint!("\r  {} ops", completed);
    }
}

// ── PUT benchmark ─────────────────────────────────────────────────────────────

pub struct PerfPutOptions {
    pub bucket: String,
    pub prefix: String,
    pub size: u64,
    pub objects: Option<u64>,
    pub duration_secs: Option<u64>,
    pub threads: usize,
    pub part_size: u64,
    pub disable_multipart: bool,
    pub json: bool,
}

pub struct PerfGetOptions {
    pub bucket: String,
    pub prefix: String,
    pub objects: Option<u64>,
    pub duration_secs: Option<u64>,
    pub threads: usize,
    pub json: bool,
}

pub struct PerfListOptions {
    pub bucket: String,
    pub prefix: String,
    pub objects: Option<u64>,
    pub duration_secs: Option<u64>,
    pub json: bool,
}

pub struct PerfDeleteOptions {
    pub bucket: String,
    pub prefix: String,
    pub objects: Option<u64>,
    pub duration_secs: Option<u64>,
    pub threads: usize,
    pub json: bool,
}

pub async fn run_put(
    client: &Client,
    opts: PerfPutOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    if !opts.json {
        println!(
            "PUT benchmark: s3://{}/{}  size={} B  threads={}  {}",
            opts.bucket,
            opts.prefix,
            opts.size,
            opts.threads,
            benchmark_mode_label(opts.objects, opts.duration_secs)
        );
    }

    // Generate random data once; clone/re-use per task.
    let data = Arc::new(generate_random_data(opts.size));
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let multipart_threshold = if opts.disable_multipart {
        u64::MAX
    } else {
        opts.part_size
    };
    let multipart = MultipartConfig {
        threshold: multipart_threshold,
        chunksize: opts.part_size,
    };
    let sse = SseConfig::default();

    let sem = Arc::new(Semaphore::new(opts.threads));
    let counter = Arc::new(AtomicU64::new(0));
    let deadline = benchmark_deadline(opts.duration_secs);

    let mut latencies: Vec<f64> = Vec::new();
    let mut tasks = tokio::task::JoinSet::new();
    let wall_start = Instant::now();
    let mut idx = 0u64;

    loop {
        // Check stop condition
        let done = match (opts.objects, deadline) {
            (Some(n), _) => counter.load(Ordering::Relaxed) >= n,
            (_, Some(dl)) => Instant::now() >= dl,
            (None, None) => false,
        };
        if done {
            break;
        }

        // Check if we've dispatched enough tasks (objects mode)
        if let Some(n) = opts.objects {
            if idx >= n {
                break;
            }
        }

        let permit = sem.clone().acquire_owned().await?;
        let client_ref = client.clone();
        let data_ref = Arc::clone(&data);
        let bucket_s = opts.bucket.clone();
        let prefix_s = opts.prefix.clone();
        let counter_ref = Arc::clone(&counter);
        let sse_clone = sse.clone();
        let multipart_config = multipart;

        let key = if prefix_s.is_empty() {
            format!("hsc-perf-{:06}-{}", idx, epoch_ms)
        } else {
            format!(
                "{}/hsc-perf-{:06}-{}",
                prefix_s.trim_end_matches('/'),
                idx,
                epoch_ms
            )
        };
        let task_idx = idx;

        idx += 1;

        tasks.spawn(async move {
            let _permit = permit;
            let t0 = Instant::now();

            // Write data to a temp file so upload_file can read it
            let tmp_path =
                std::env::temp_dir().join(format!("hsc-perf-put-{}-{}.dat", epoch_ms, task_idx));
            tokio::fs::write(&tmp_path, data_ref.as_ref()).await?;

            let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = upload_file(
                &client_ref,
                tmp_path.to_str().unwrap(),
                &bucket_s,
                &key,
                UploadOptions {
                    checksum_mode: None,
                    checksum_algorithm: None,
                    sse: &sse_clone,
                    multipart: multipart_config,
                    #[cfg(feature = "rdma")]
                    rdma: None,
                },
            )
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() });

            let _ = tokio::fs::remove_file(&tmp_path).await;
            result?;

            counter_ref.fetch_add(1, Ordering::Relaxed);
            Ok::<f64, Box<dyn std::error::Error + Send + Sync>>(t0.elapsed().as_secs_f64() * 1000.0)
        });

        // Drain completed tasks when the semaphore is nearly full to keep
        // memory bounded and collect latencies continuously.
        while let Some(res) = tasks.try_join_next() {
            match res {
                Ok(Ok(lat)) => {
                    latencies.push(lat);
                    if !opts.json {
                        print_progress(counter.load(Ordering::Relaxed), opts.objects);
                    }
                }
                Ok(Err(e)) => eprintln!("\nPUT error: {}", e),
                Err(e) => eprintln!("\nTask error: {}", e),
            }
        }
    }

    // Wait for remaining tasks
    while let Some(res) = tasks.join_next().await {
        match res {
            Ok(Ok(lat)) => {
                latencies.push(lat);
                if !opts.json {
                    print_progress(counter.load(Ordering::Relaxed), opts.objects);
                }
            }
            Ok(Err(e)) => eprintln!("\nPUT error: {}", e),
            Err(e) => eprintln!("\nTask error: {}", e),
        }
    }

    let elapsed = wall_start.elapsed().as_secs_f64();
    let ops = counter.load(Ordering::Relaxed);
    let ops_per_sec = calculate_ops_per_sec(ops, elapsed);
    let mb_per_sec = ops_per_sec * opts.size as f64 / (1024.0 * 1024.0);
    let metrics = benchmark_metrics(ops, elapsed, latencies, Some(mb_per_sec));

    if opts.json {
        let result = BenchmarkResult::new(
            "put",
            opts.bucket,
            opts.prefix,
            opts.threads,
            Some(opts.size),
            metrics,
        );
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_benchmark_results(&metrics);
    }

    Ok(())
}

// ── GET benchmark ─────────────────────────────────────────────────────────────

pub async fn run_get(
    client: &Client,
    opts: PerfGetOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let limit = opts.objects.unwrap_or(100);

    if !opts.json {
        println!(
            "GET benchmark: s3://{}/{}  threads={}  {}",
            opts.bucket,
            opts.prefix,
            opts.threads,
            benchmark_mode_label(opts.objects, opts.duration_secs)
        );
        eprintln!("  Listing objects...");
    }

    let keys = list_s3_keys_limited(client, &opts.bucket, &opts.prefix, limit).await?;
    if keys.is_empty() {
        return Err(format!("No objects found at s3://{}/{}", opts.bucket, opts.prefix).into());
    }
    if !opts.json {
        eprintln!("  Found {} objects", keys.len());
    }

    let sem = Arc::new(Semaphore::new(opts.threads));
    let counter = Arc::new(AtomicU64::new(0));
    let deadline = benchmark_deadline(opts.duration_secs);

    let mut latencies: Vec<f64> = Vec::new();
    let mut tasks = tokio::task::JoinSet::new();
    let wall_start = Instant::now();
    let mut key_idx = 0usize;
    let mut dispatched = 0u64;

    loop {
        let done = match (opts.objects, deadline) {
            (Some(n), _) => counter.load(Ordering::Relaxed) >= n,
            (_, Some(dl)) => Instant::now() >= dl,
            (None, None) => false,
        };
        if done {
            break;
        }
        if let Some(n) = opts.objects {
            if dispatched >= n {
                break;
            }
        }

        let permit = sem.clone().acquire_owned().await?;
        let client_ref = client.clone();
        let bucket_s = opts.bucket.clone();
        let key = keys[key_idx % keys.len()].clone();
        key_idx += 1;
        dispatched += 1;
        let counter_ref = Arc::clone(&counter);

        tasks.spawn(async move {
            let _permit = permit;
            let t0 = Instant::now();

            let resp = client_ref
                .get_object()
                .bucket(&bucket_s)
                .key(&key)
                .send()
                .await?;

            // Drain the body so the transfer is actually measured
            let mut body = resp.body;
            let mut byte_count = 0u64;
            while let Some(chunk) = body.try_next().await? {
                byte_count += chunk.len() as u64;
            }

            counter_ref.fetch_add(1, Ordering::Relaxed);
            Ok::<(f64, u64), Box<dyn std::error::Error + Send + Sync>>((
                t0.elapsed().as_secs_f64() * 1000.0,
                byte_count,
            ))
        });

        while let Some(res) = tasks.try_join_next() {
            match res {
                Ok(Ok((lat, _bytes))) => {
                    latencies.push(lat);
                    if !opts.json {
                        print_progress(counter.load(Ordering::Relaxed), opts.objects);
                    }
                }
                Ok(Err(e)) => eprintln!("\nGET error: {}", e),
                Err(e) => eprintln!("\nTask error: {}", e),
            }
        }
    }

    while let Some(res) = tasks.join_next().await {
        match res {
            Ok(Ok((lat, _bytes))) => {
                latencies.push(lat);
                if !opts.json {
                    print_progress(counter.load(Ordering::Relaxed), opts.objects);
                }
            }
            Ok(Err(e)) => eprintln!("\nGET error: {}", e),
            Err(e) => eprintln!("\nTask error: {}", e),
        }
    }

    let elapsed = wall_start.elapsed().as_secs_f64();
    let ops = counter.load(Ordering::Relaxed);
    let ops_per_sec = calculate_ops_per_sec(ops, elapsed);

    // Estimate per-object size from first object's head (best-effort)
    let obj_size = match client
        .head_object()
        .bucket(&opts.bucket)
        .key(&keys[0])
        .send()
        .await
    {
        Ok(h) => h.content_length().unwrap_or(0) as u64,
        Err(_) => 0,
    };
    let mb_per_sec = if obj_size > 0 {
        Some(ops_per_sec * obj_size as f64 / (1024.0 * 1024.0))
    } else {
        None
    };
    let metrics = benchmark_metrics(ops, elapsed, latencies, mb_per_sec);

    if opts.json {
        let result = BenchmarkResult::new(
            "get",
            opts.bucket,
            opts.prefix,
            opts.threads,
            if obj_size > 0 { Some(obj_size) } else { None },
            metrics,
        );
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_benchmark_results(&metrics);
    }

    Ok(())
}

// ── LIST benchmark ────────────────────────────────────────────────────────────

pub async fn run_list(
    client: &Client,
    opts: PerfListOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    if !opts.json {
        println!(
            "LIST benchmark: s3://{}/{}  {}",
            opts.bucket,
            opts.prefix,
            benchmark_mode_label(opts.objects, opts.duration_secs)
        );
    }

    let deadline = benchmark_deadline(opts.duration_secs);
    let limit = opts.objects.unwrap_or(100);

    let mut latencies: Vec<f64> = Vec::new();
    let mut ops: u64 = 0;
    let wall_start = Instant::now();

    loop {
        let done = match (opts.objects, deadline) {
            (Some(n), _) => ops >= n,
            (_, Some(dl)) => Instant::now() >= dl,
            (None, None) => ops >= limit,
        };
        if done {
            break;
        }

        let t0 = Instant::now();
        let mut req = client.list_objects_v2().bucket(&opts.bucket).max_keys(1000);
        if !opts.prefix.is_empty() {
            req = req.prefix(&opts.prefix);
        }
        req.send().await?;

        latencies.push(t0.elapsed().as_secs_f64() * 1000.0);
        ops += 1;

        if !opts.json {
            print_progress(ops, opts.objects);
        }
    }

    let elapsed = wall_start.elapsed().as_secs_f64();
    let metrics = benchmark_metrics(ops, elapsed, latencies, None);

    if opts.json {
        let result = BenchmarkResult::new("list", opts.bucket, opts.prefix, 1, None, metrics);
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_benchmark_results(&metrics);
    }

    Ok(())
}

// ── DELETE benchmark ──────────────────────────────────────────────────────────

pub async fn run_delete(
    client: &Client,
    opts: PerfDeleteOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let limit = opts.objects.unwrap_or(100);

    if !opts.json {
        println!(
            "DELETE benchmark: s3://{}/{}  threads={}  {}",
            opts.bucket,
            opts.prefix,
            opts.threads,
            benchmark_mode_label(opts.objects, opts.duration_secs)
        );
        eprintln!("  Listing objects...");
    }

    let keys = list_s3_keys_limited(client, &opts.bucket, &opts.prefix, limit).await?;
    if keys.is_empty() {
        return Err(format!("No objects found at s3://{}/{}", opts.bucket, opts.prefix).into());
    }
    if !opts.json {
        eprintln!("  Found {} objects to delete", keys.len());
    }

    // Batch into chunks of 1000 (S3 limit per DeleteObjects call)
    let batches: Vec<Vec<String>> = keys.chunks(1000).map(|c| c.to_vec()).collect();

    let sem = Arc::new(Semaphore::new(opts.threads));
    let counter = Arc::new(AtomicU64::new(0));
    let wall_start = Instant::now();

    let mut latencies: Vec<f64> = Vec::new();
    let mut tasks = tokio::task::JoinSet::new();

    for batch in batches {
        let permit = sem.clone().acquire_owned().await?;
        let client_ref = client.clone();
        let bucket_s = opts.bucket.clone();
        let batch_len = batch.len() as u64;
        let counter_ref = Arc::clone(&counter);

        tasks.spawn(async move {
            let _permit = permit;
            let t0 = Instant::now();

            let objects: Vec<ObjectIdentifier> = batch
                .iter()
                .filter_map(|k| ObjectIdentifier::builder().key(k).build().ok())
                .collect();

            let delete = Delete::builder()
                .set_objects(Some(objects))
                .quiet(true)
                .build()?;

            client_ref
                .delete_objects()
                .bucket(&bucket_s)
                .delete(delete)
                .send()
                .await?;

            counter_ref.fetch_add(batch_len, Ordering::Relaxed);
            Ok::<f64, Box<dyn std::error::Error + Send + Sync>>(t0.elapsed().as_secs_f64() * 1000.0)
        });

        while let Some(res) = tasks.try_join_next() {
            match res {
                Ok(Ok(lat)) => {
                    latencies.push(lat);
                    if !opts.json {
                        print_progress(counter.load(Ordering::Relaxed), opts.objects);
                    }
                }
                Ok(Err(e)) => eprintln!("\nDELETE error: {}", e),
                Err(e) => eprintln!("\nTask error: {}", e),
            }
        }
    }

    while let Some(res) = tasks.join_next().await {
        match res {
            Ok(Ok(lat)) => {
                latencies.push(lat);
                if !opts.json {
                    print_progress(counter.load(Ordering::Relaxed), opts.objects);
                }
            }
            Ok(Err(e)) => eprintln!("\nDELETE error: {}", e),
            Err(e) => eprintln!("\nTask error: {}", e),
        }
    }

    let elapsed = wall_start.elapsed().as_secs_f64();
    let ops = counter.load(Ordering::Relaxed);
    let metrics = benchmark_metrics(ops, elapsed, latencies, None);

    if opts.json {
        let result = BenchmarkResult::new(
            "delete",
            opts.bucket,
            opts.prefix,
            opts.threads,
            None,
            metrics,
        );
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print_benchmark_results(&metrics);
    }

    Ok(())
}
