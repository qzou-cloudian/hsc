use crate::commands::listing::{list_s3_objects, walk_local_files};
use crate::filters::FileFilter;
use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use md5::{Digest, Md5};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone)]
struct FileInfo {
    size: u64,
    etag: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
enum DiffType {
    OnlyInSource,
    OnlyInDest,
    SizeDiffers,
    ContentDiffers,
}

#[derive(Serialize)]
struct DiffEntry {
    path: String,
    kind: DiffType,
}

#[derive(Serialize)]
struct DiffSummary {
    only_in_source: usize,
    only_in_destination: usize,
    size_differs: usize,
    content_differs: usize,
    total_differences: usize,
}

#[derive(Serialize)]
struct DiffOutput {
    source: String,
    dest: String,
    identical: bool,
    differences: Vec<DiffEntry>,
    summary: DiffSummary,
}

/// Compare two directories or buckets and show differences
pub async fn diff(
    client: &Client,
    source: &str,
    dest: &str,
    compare_content: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_type = parse_path(source)?;
    let dest_type = parse_path(dest)?;

    let filter = FileFilter::new(include, exclude)?;

    // Collect file information from both source and dest
    let source_files = collect_files(client, &source_type, &filter, compare_content).await?;
    let dest_files = collect_files(client, &dest_type, &filter, compare_content).await?;

    // Find differences
    let differences = find_differences(&source_files, &dest_files, compare_content);

    let output = build_output(source, dest, &differences);
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        display_differences(&output);
    }

    Ok(())
}

/// Collect files from a path (local or S3)
async fn collect_files(
    client: &Client,
    path_type: &PathType,
    filter: &FileFilter,
    calculate_etag: bool,
) -> Result<HashMap<String, FileInfo>, Box<dyn std::error::Error>> {
    match path_type {
        PathType::S3 { bucket, key } => {
            collect_s3_files(client, bucket, key, filter, calculate_etag).await
        }
        PathType::Local(path) => collect_local_files(path, filter, calculate_etag).await,
    }
}

/// Collect files from S3
async fn collect_s3_files(
    client: &Client,
    bucket: &str,
    prefix: &str,
    filter: &FileFilter,
    _calculate_etag: bool,
) -> Result<HashMap<String, FileInfo>, Box<dyn std::error::Error>> {
    let mut files = HashMap::new();

    for obj in list_s3_objects(client, bucket, prefix).await? {
        let relative_key = obj.relative_key(prefix).to_string();

        if relative_key.is_empty() {
            continue;
        }

        // Apply filters
        if !filter.matches(&relative_key) {
            continue;
        }

        let etag = obj.etag.map(|s| s.trim_matches('"').to_string());

        files.insert(
            relative_key,
            FileInfo {
                size: obj.size as u64,
                etag,
            },
        );
    }

    Ok(files)
}

/// Collect files from local filesystem
async fn collect_local_files(
    path: &str,
    filter: &FileFilter,
    calculate_etag: bool,
) -> Result<HashMap<String, FileInfo>, Box<dyn std::error::Error>> {
    let mut files = HashMap::new();
    let base_path = Path::new(path);

    if !base_path.exists() {
        return Err(format!("Path '{}' does not exist", path).into());
    }

    if base_path.is_file() {
        // Single file
        let metadata = fs::metadata(base_path).await?;
        let etag = if calculate_etag {
            calculate_file_etag(base_path).await.ok()
        } else {
            None
        };

        let file_name = base_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if filter.matches(&file_name) {
            files.insert(
                file_name.clone(),
                FileInfo {
                    size: metadata.len(),
                    etag,
                },
            );
        }
    } else {
        // Directory - walk recursively
        for entry in walk_local_files(path)? {
            if !filter.matches(&entry.relative) {
                continue;
            }

            let metadata = fs::metadata(&entry.path).await?;
            let etag = if calculate_etag {
                calculate_file_etag(&entry.path).await.ok()
            } else {
                None
            };

            files.insert(
                entry.relative,
                FileInfo {
                    size: metadata.len(),
                    etag,
                },
            );
        }
    }

    Ok(files)
}

/// Calculate MD5 hash (ETag) of a file
async fn calculate_file_etag(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Md5::new();
    let mut buffer = vec![0u8; 8192];

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Find differences between source and destination
fn find_differences(
    source_files: &HashMap<String, FileInfo>,
    dest_files: &HashMap<String, FileInfo>,
    compare_content: bool,
) -> Vec<(String, DiffType)> {
    let mut differences = Vec::new();

    // Get all unique file paths
    let mut all_paths: HashSet<String> = source_files.keys().cloned().collect();
    all_paths.extend(dest_files.keys().cloned());

    let mut sorted_paths: Vec<String> = all_paths.into_iter().collect();
    sorted_paths.sort();

    for path in sorted_paths {
        let source_info = source_files.get(&path);
        let dest_info = dest_files.get(&path);

        match (source_info, dest_info) {
            (Some(_), None) => {
                differences.push((path, DiffType::OnlyInSource));
            }
            (None, Some(_)) => {
                differences.push((path, DiffType::OnlyInDest));
            }
            (Some(src), Some(dst)) => {
                // Both exist - check if they differ
                if src.size != dst.size {
                    differences.push((path, DiffType::SizeDiffers));
                } else if compare_content {
                    // Compare ETags if available
                    if let (Some(src_etag), Some(dst_etag)) = (&src.etag, &dst.etag) {
                        if src_etag != dst_etag {
                            differences.push((path, DiffType::ContentDiffers));
                        }
                    }
                }
            }
            (None, None) => {
                // This shouldn't happen
            }
        }
    }

    differences
}

/// Display differences in a readable format
fn build_output(source: &str, dest: &str, differences: &[(String, DiffType)]) -> DiffOutput {
    let mut entries = Vec::with_capacity(differences.len());
    let mut only_source = 0usize;
    let mut only_dest = 0usize;
    let mut size_differs = 0usize;
    let mut content_differs = 0usize;

    for (path, diff_type) in differences {
        match diff_type {
            DiffType::OnlyInSource => only_source += 1,
            DiffType::OnlyInDest => only_dest += 1,
            DiffType::SizeDiffers => size_differs += 1,
            DiffType::ContentDiffers => content_differs += 1,
        }
        entries.push(DiffEntry {
            path: path.clone(),
            kind: diff_type.clone(),
        });
    }

    DiffOutput {
        source: source.to_string(),
        dest: dest.to_string(),
        identical: differences.is_empty(),
        differences: entries,
        summary: DiffSummary {
            only_in_source: only_source,
            only_in_destination: only_dest,
            size_differs,
            content_differs,
            total_differences: differences.len(),
        },
    }
}

fn display_differences(output: &DiffOutput) {
    if output.identical {
        println!("No differences found between:");
        println!("  Source: {}", output.source);
        println!("  Dest:   {}", output.dest);
        return;
    }

    println!("Differences between:");
    println!("  Source: {}", output.source);
    println!("  Dest:   {}", output.dest);
    println!();

    let mut only_source = Vec::new();
    let mut only_dest = Vec::new();
    let mut size_differs = Vec::new();
    let mut content_differs = Vec::new();

    for entry in &output.differences {
        match entry.kind {
            DiffType::OnlyInSource => only_source.push(entry.path.as_str()),
            DiffType::OnlyInDest => only_dest.push(entry.path.as_str()),
            DiffType::SizeDiffers => size_differs.push(entry.path.as_str()),
            DiffType::ContentDiffers => content_differs.push(entry.path.as_str()),
        }
    }

    if !only_source.is_empty() {
        println!("Only in source ({} files):", only_source.len());
        for path in &only_source {
            println!("  + {}", path);
        }
        println!();
    }

    if !only_dest.is_empty() {
        println!("Only in destination ({} files):", only_dest.len());
        for path in &only_dest {
            println!("  - {}", path);
        }
        println!();
    }

    if !size_differs.is_empty() {
        println!("Size differs ({} files):", size_differs.len());
        for path in &size_differs {
            println!("  ≠ {}", path);
        }
        println!();
    }

    if !content_differs.is_empty() {
        println!("Content differs ({} files):", content_differs.len());
        for path in &content_differs {
            println!("  ≠ {}", path);
        }
        println!();
    }

    println!("Summary:");
    println!("  Only in source:      {}", output.summary.only_in_source);
    println!(
        "  Only in destination: {}",
        output.summary.only_in_destination
    );
    println!("  Size differs:        {}", output.summary.size_differs);
    println!("  Content differs:     {}", output.summary.content_differs);
    println!(
        "  Total differences:   {}",
        output.summary.total_differences
    );
}
