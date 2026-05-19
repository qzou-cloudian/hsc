use aws_sdk_s3::operation::head_object::HeadObjectOutput;
use aws_sdk_s3::types::ObjectPart;
use serde::Serialize;
use serde_json::{json, Map, Value};

#[derive(Debug, Default, Clone, Serialize)]
pub(crate) struct ObjectChecksums {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_crc32: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_crc32c: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_crc64nvme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_sha1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_sha256: Option<String>,
}

impl ObjectChecksums {
    pub(crate) fn from_head_object(response: &HeadObjectOutput) -> Self {
        Self {
            checksum_crc32: response.checksum_crc32().map(str::to_string),
            checksum_crc32c: response.checksum_crc32_c().map(str::to_string),
            checksum_crc64nvme: response.checksum_crc64_nvme().map(str::to_string),
            checksum_sha1: response.checksum_sha1().map(str::to_string),
            checksum_sha256: response.checksum_sha256().map(str::to_string),
        }
    }

    pub(crate) fn from_object_part(part: &ObjectPart) -> Self {
        Self {
            checksum_crc32: part.checksum_crc32().map(str::to_string),
            checksum_crc32c: part.checksum_crc32_c().map(str::to_string),
            checksum_crc64nvme: part.checksum_crc64_nvme().map(str::to_string),
            checksum_sha1: part.checksum_sha1().map(str::to_string),
            checksum_sha256: part.checksum_sha256().map(str::to_string),
        }
    }

    pub(crate) fn has_any(&self) -> bool {
        self.checksum_crc32.is_some()
            || self.checksum_crc32c.is_some()
            || self.checksum_crc64nvme.is_some()
            || self.checksum_sha1.is_some()
            || self.checksum_sha256.is_some()
    }

    pub(crate) fn print_stat_lines(&self) {
        if let Some(checksum) = &self.checksum_crc32 {
            println!("CRC32     : {}", checksum);
        }
        if let Some(checksum) = &self.checksum_crc32c {
            println!("CRC32C    : {}", checksum);
        }
        if let Some(checksum) = &self.checksum_sha1 {
            println!("SHA1      : {}", checksum);
        }
        if let Some(checksum) = &self.checksum_sha256 {
            println!("SHA256    : {}", checksum);
        }
    }

    pub(crate) fn insert_stat_json(&self, out: &mut Map<String, Value>) {
        if let Some(checksum) = &self.checksum_crc32 {
            out.insert("crc32".to_string(), json!(checksum));
        }
        if let Some(checksum) = &self.checksum_crc32c {
            out.insert("crc32c".to_string(), json!(checksum));
        }
        if let Some(checksum) = &self.checksum_sha1 {
            out.insert("sha1".to_string(), json!(checksum));
        }
        if let Some(checksum) = &self.checksum_sha256 {
            out.insert("sha256".to_string(), json!(checksum));
        }
    }

    pub(crate) fn table_values(&self) -> [&str; 5] {
        [
            self.checksum_crc32.as_deref().unwrap_or("-"),
            self.checksum_crc32c.as_deref().unwrap_or("-"),
            self.checksum_crc64nvme.as_deref().unwrap_or("-"),
            self.checksum_sha1.as_deref().unwrap_or("-"),
            self.checksum_sha256.as_deref().unwrap_or("-"),
        ]
    }
}
