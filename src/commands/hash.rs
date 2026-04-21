use crate::path_utils::{parse_path, PathType};
use aws_sdk_s3::Client;
use crc32fast::Hasher as Crc32Hasher;
use md5::{Digest, Md5};
use serde::Serialize;
use sha1::Sha1;
use sha2::Sha256;
use tokio::fs;
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone, Serialize)]
pub struct HashOutput {
    pub path: String,
    pub algorithm: String,
    pub value: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
pub enum HashAlgorithm {
    Md5,
    Crc32,
    Crc32c,
    Sha1,
    Sha256,
}

impl HashAlgorithm {
    pub fn parse(input: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match input.trim().to_ascii_uppercase().as_str() {
            "MD5" => Ok(Self::Md5),
            "CRC32" => Ok(Self::Crc32),
            "CRC32C" => Ok(Self::Crc32c),
            "SHA1" => Ok(Self::Sha1),
            "SHA256" => Ok(Self::Sha256),
            other => Err(format!(
                "Unsupported hash algorithm '{}'. Expected one of: MD5, CRC32, CRC32C, SHA1, SHA256",
                other
            )
            .into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Crc32 => "CRC32",
            Self::Crc32c => "CRC32C",
            Self::Sha1 => "SHA1",
            Self::Sha256 => "SHA256",
        }
    }
}

enum HasherState {
    Md5(Md5),
    Crc32(Crc32Hasher),
    Crc32c(u32),
    Sha1(Sha1),
    Sha256(Sha256),
}

impl HasherState {
    fn new(algorithm: HashAlgorithm) -> Self {
        match algorithm {
            HashAlgorithm::Md5 => Self::Md5(Md5::new()),
            HashAlgorithm::Crc32 => Self::Crc32(Crc32Hasher::new()),
            HashAlgorithm::Crc32c => Self::Crc32c(0),
            HashAlgorithm::Sha1 => Self::Sha1(Sha1::new()),
            HashAlgorithm::Sha256 => Self::Sha256(Sha256::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Md5(hasher) => hasher.update(bytes),
            Self::Crc32(hasher) => hasher.update(bytes),
            Self::Crc32c(state) => *state = crc32c::crc32c_append(*state, bytes),
            Self::Sha1(hasher) => hasher.update(bytes),
            Self::Sha256(hasher) => hasher.update(bytes),
        }
    }

    fn finalize(self) -> String {
        match self {
            Self::Md5(hasher) => format!("{:x}", hasher.finalize()),
            Self::Crc32(hasher) => format!("{:08x}", hasher.finalize()),
            Self::Crc32c(state) => format!("{:08x}", state),
            Self::Sha1(hasher) => format!("{:x}", hasher.finalize()),
            Self::Sha256(hasher) => format!("{:x}", hasher.finalize()),
        }
    }
}

pub async fn hash(
    client: &Client,
    path: &str,
    algorithm: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = compute_hash_for_path(client, path, algorithm).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}\t{}\t{}", result.algorithm, result.value, result.path);
    }
    Ok(())
}

pub async fn compute_hash_for_path(
    client: &Client,
    path: &str,
    algorithm: &str,
) -> Result<HashOutput, Box<dyn std::error::Error>> {
    let algorithm = HashAlgorithm::parse(algorithm)?;
    let mut hasher = HasherState::new(algorithm);
    let mut size = 0u64;

    match parse_path(path)? {
        PathType::Local(local_path) => {
            let mut file = fs::File::open(&local_path)
                .await
                .map_err(|e| format!("Cannot open '{}': {}", local_path, e))?;
            let metadata = file.metadata().await?;
            if !metadata.is_file() {
                return Err(format!("'{}' is not a file", local_path).into());
            }

            let mut buffer = vec![0u8; 8192];
            loop {
                let n = file.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
                size += n as u64;
            }
        }
        PathType::S3 { bucket, key } => {
            if key.is_empty() {
                return Err(format!("'{}' is an S3 bucket, not an object", path).into());
            }
            let response = client.get_object().bucket(&bucket).key(&key).send().await?;
            let mut body = response.body;
            while let Some(bytes) = body.try_next().await? {
                hasher.update(&bytes);
                size += bytes.len() as u64;
            }
        }
    }

    Ok(HashOutput {
        path: path.to_string(),
        algorithm: algorithm.name().to_string(),
        value: hasher.finalize(),
        size,
    })
}
