use crate::filters::FileFilter;
use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use md5::{Digest, Md5};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FileInfo {
    path: String,
    size: u64,
    etag: Option<String>,
}

#[derive(Debug)]
enum DiffType {
    OnlyInSource,
    OnlyInDest,
    SizeDiffers,
    ContentDiffers,
}

/// Compare two directories or buckets and show differences
pub async fn diff(
    client: &Client,
    source: &str,
    dest: &str,
    compare_content: bool,
    include: Vec<String>,
    exclude: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let source_type = parse_path(source)?;
    let dest_type = parse_path(dest)?;

    let filter = FileFilter::new(include, exclude)?;

    // Collect file information from both source and dest
    let source_files = collect_files(client, &source_type, &filter, compare_content).await?;
    let dest_files = collect_files(client, &dest_type, &filter, compare_content).await?;

    // Find differences
    let differences = find_differences(&source_files, &dest_files, compare_content);

    // Display results
    display_differences(source, dest, &differences);

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
    let mut continuation_token: Option<String> = None;

    loop {
        let mut request = client.list_objects_v2().bucket(bucket);

        if !prefix.is_empty() {
            request = request.prefix(prefix);
        }

        if let Some(token) = continuation_token {
            request = request.continuation_token(token);
        }

        let response = request.send().await?;

        for obj in response.contents() {
            if let Some(key) = obj.key() {
                // Get relative path (remove prefix)
                let relative_key = if !prefix.is_empty() && key.starts_with(prefix) {
                    key[prefix.len()..].trim_start_matches('/')
                } else {
                    key
                };

                if relative_key.is_empty() {
                    continue;
                }

                // Apply filters
                if !filter.matches(relative_key) {
                    continue;
                }

                let size = obj.size().unwrap_or(0) as u64;
                let etag = obj.e_tag().map(|s| s.trim_matches('"').to_string());

                files.insert(
                    relative_key.to_string(),
                    FileInfo {
                        path: key.to_string(),
                        size,
                        etag,
                    },
                );
            }
        }

        if response.is_truncated() == Some(true) {
            continuation_token = response.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
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
                    path: path.to_string(),
                    size: metadata.len(),
                    etag,
                },
            );
        }
    } else {
        // Directory - walk recursively
        for entry in WalkDir::new(base_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let full_path = entry.path();
            let relative_path = full_path
                .strip_prefix(base_path)
                .unwrap_or(full_path)
                .to_string_lossy()
                .to_string();

            if !filter.matches(&relative_path) {
                continue;
            }

            let metadata = fs::metadata(full_path).await?;
            let etag = if calculate_etag {
                calculate_file_etag(full_path).await.ok()
            } else {
                None
            };

            files.insert(
                relative_path,
                FileInfo {
                    path: full_path.to_string_lossy().to_string(),
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
fn display_differences(source: &str, dest: &str, differences: &[(String, DiffType)]) {
    if differences.is_empty() {
        println!("No differences found between:");
        println!("  Source: {}", source);
        println!("  Dest:   {}", dest);
        return;
    }

    println!("Differences between:");
    println!("  Source: {}", source);
    println!("  Dest:   {}", dest);
    println!();

    let mut only_source = Vec::new();
    let mut only_dest = Vec::new();
    let mut size_differs = Vec::new();
    let mut content_differs = Vec::new();

    for (path, diff_type) in differences {
        match diff_type {
            DiffType::OnlyInSource => only_source.push(path),
            DiffType::OnlyInDest => only_dest.push(path),
            DiffType::SizeDiffers => size_differs.push(path),
            DiffType::ContentDiffers => content_differs.push(path),
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
    println!("  Only in source:      {}", only_source.len());
    println!("  Only in destination: {}", only_dest.len());
    println!("  Size differs:        {}", size_differs.len());
    println!("  Content differs:     {}", content_differs.len());
    println!("  Total differences:   {}", differences.len());
}
