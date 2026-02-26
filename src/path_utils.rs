use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum PathType {
    Local(String),
    S3 { bucket: String, key: String },
}

/// Parse a path string into PathType
pub fn parse_path(path: &str) -> Result<PathType, String> {
    if path.starts_with("s3://") {
        parse_s3_uri(path)
    } else {
        Ok(PathType::Local(path.to_string()))
    }
}

/// Parse an S3 URI in the format s3://bucket/key or s3://bucket
pub fn parse_s3_uri(uri: &str) -> Result<PathType, String> {
    if !uri.starts_with("s3://") {
        return Err(format!("Invalid S3 URI: {}", uri));
    }

    let path = &uri[5..]; // Remove "s3://"

    if path.is_empty() {
        return Err("S3 URI must contain a bucket name".to_string());
    }

    let parts: Vec<&str> = path.splitn(2, '/').collect();
    let bucket = parts[0].to_string();

    if bucket.is_empty() {
        return Err("Bucket name cannot be empty".to_string());
    }

    let key = if parts.len() > 1 {
        parts[1].to_string()
    } else {
        String::new()
    };

    Ok(PathType::S3 { bucket, key })
}

/// Check if a path is a local directory
#[allow(dead_code)]
pub fn is_local_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// Check if a path is a local file
#[allow(dead_code)]
pub fn is_local_file(path: &str) -> bool {
    Path::new(path).is_file()
}

/// Normalize S3 key by removing leading slashes
pub fn normalize_s3_key(key: &str) -> String {
    key.trim_start_matches('/').to_string()
}

/// Join S3 key components
pub fn join_s3_key(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        normalize_s3_key(name)
    } else if prefix.ends_with('/') {
        format!("{}{}", prefix, name.trim_start_matches('/'))
    } else {
        format!("{}/{}", prefix, name.trim_start_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_s3_uri() {
        let result = parse_s3_uri("s3://my-bucket/path/to/file.txt");
        assert!(result.is_ok());
        if let PathType::S3 { bucket, key } = result.unwrap() {
            assert_eq!(bucket, "my-bucket");
            assert_eq!(key, "path/to/file.txt");
        }

        let result = parse_s3_uri("s3://my-bucket");
        assert!(result.is_ok());
        if let PathType::S3 { bucket, key } = result.unwrap() {
            assert_eq!(bucket, "my-bucket");
            assert_eq!(key, "");
        }
    }

    #[test]
    fn test_join_s3_key() {
        assert_eq!(join_s3_key("prefix", "file.txt"), "prefix/file.txt");
        assert_eq!(join_s3_key("prefix/", "file.txt"), "prefix/file.txt");
        assert_eq!(join_s3_key("", "file.txt"), "file.txt");
    }
}
