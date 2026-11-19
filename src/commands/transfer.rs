use aws_sdk_s3::operation::copy_object::builders::CopyObjectFluentBuilder;
use aws_sdk_s3::operation::create_multipart_upload::builders::CreateMultipartUploadFluentBuilder;
use aws_sdk_s3::operation::get_object::builders::GetObjectFluentBuilder;
use aws_sdk_s3::operation::head_object::builders::HeadObjectFluentBuilder;
use aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder;
use aws_sdk_s3::operation::upload_part::builders::UploadPartFluentBuilder;
use aws_sdk_s3::operation::upload_part::UploadPartOutput;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumAlgorithm, ChecksumMode, CompletedPart, ServerSideEncryption};

use base64::{engine::general_purpose::STANDARD, Engine as _};

#[cfg(feature = "rdma")]
use crate::rdma::{RdmaClientChannel, RdmaClientProvider, RdmaInterceptor, RdmaTransferHandle};
#[cfg(feature = "rdma")]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub(crate) struct SseCustomerHeaders {
    pub algorithm: String,
    pub key: Option<String>,
    pub key_md5: Option<String>,
}

/// Server-side encryption options, matching AWS S3 CLI conventions.
#[derive(Clone, Default, Debug)]
pub struct SseConfig {
    /// `--sse`: destination SSE algorithm ("AES256", "aws:kms", "aws:kms:dsse")
    pub sse: Option<String>,
    /// `--sse-kms-key-id`: AWS KMS key ARN or alias
    pub sse_kms_key_id: Option<String>,
    /// `--sse-c`: SSE-C algorithm for the destination object (typically "AES256")
    pub sse_c: Option<String>,
    /// `--sse-c-key`: base64-encoded 256-bit customer-provided key for the destination
    pub sse_c_key: Option<String>,
    /// `--sse-c-copy-source`: SSE-C algorithm for the copy source (S3-to-S3 only)
    pub sse_c_copy_source: Option<String>,
    /// `--sse-c-copy-source-key`: base64-encoded customer-provided key for the copy source
    pub sse_c_copy_source_key: Option<String>,
}

impl SseConfig {
    pub(crate) fn destination_customer_headers(
        &self,
    ) -> Result<Option<SseCustomerHeaders>, Box<dyn std::error::Error>> {
        sse_customer_headers(self.sse_c.as_deref(), self.sse_c_key.as_deref())
    }

    pub(crate) fn copy_source_customer_headers(
        &self,
    ) -> Result<Option<SseCustomerHeaders>, Box<dyn std::error::Error>> {
        sse_customer_headers(
            self.sse_c_copy_source.as_deref(),
            self.sse_c_copy_source_key.as_deref(),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MultipartConfig {
    pub threshold: u64,
    pub chunksize: u64,
}

pub(crate) fn sse_customer_headers(
    algorithm: Option<&str>,
    key_b64: Option<&str>,
) -> Result<Option<SseCustomerHeaders>, Box<dyn std::error::Error>> {
    if algorithm.is_none() && key_b64.is_none() {
        return Ok(None);
    }

    Ok(Some(SseCustomerHeaders {
        algorithm: algorithm.unwrap_or("AES256").to_string(),
        key: key_b64.map(ToOwned::to_owned),
        key_md5: key_b64.map(sse_c_key_md5).transpose()?,
    }))
}

/// Compute `base64(MD5(raw_key_bytes))` required by S3 for SSE-C requests.
pub(crate) fn sse_c_key_md5(key_b64: &str) -> Result<String, Box<dyn std::error::Error>> {
    use md5::{Digest, Md5};
    let raw = STANDARD.decode(key_b64)?;
    let digest = Md5::digest(&raw);
    Ok(STANDARD.encode(digest))
}

macro_rules! apply_destination_sse_customer {
    ($req:expr, $customer:expr) => {{
        let mut req = $req;
        if let Some(customer) = $customer {
            req = req.sse_customer_algorithm(customer.algorithm);
            if let (Some(key), Some(key_md5)) = (customer.key, customer.key_md5) {
                req = req.sse_customer_key(key).sse_customer_key_md5(key_md5);
            }
        }
        req
    }};
}

pub(crate) fn apply_sse_customer_to_put_object(
    req: PutObjectFluentBuilder,
    customer: Option<SseCustomerHeaders>,
) -> PutObjectFluentBuilder {
    apply_destination_sse_customer!(req, customer)
}

pub(crate) fn apply_sse_customer_to_upload_part(
    req: UploadPartFluentBuilder,
    customer: Option<SseCustomerHeaders>,
) -> UploadPartFluentBuilder {
    apply_destination_sse_customer!(req, customer)
}

pub(crate) fn apply_sse_customer_to_create_multipart(
    req: CreateMultipartUploadFluentBuilder,
    customer: Option<SseCustomerHeaders>,
) -> CreateMultipartUploadFluentBuilder {
    apply_destination_sse_customer!(req, customer)
}

pub(crate) fn apply_sse_customer_to_get_object(
    req: GetObjectFluentBuilder,
    customer: Option<SseCustomerHeaders>,
) -> GetObjectFluentBuilder {
    apply_destination_sse_customer!(req, customer)
}

pub(crate) fn apply_sse_customer_to_head_object(
    req: HeadObjectFluentBuilder,
    customer: Option<SseCustomerHeaders>,
) -> HeadObjectFluentBuilder {
    apply_destination_sse_customer!(req, customer)
}

pub(crate) fn apply_destination_sse_customer_to_copy_object(
    req: CopyObjectFluentBuilder,
    customer: Option<SseCustomerHeaders>,
) -> CopyObjectFluentBuilder {
    apply_destination_sse_customer!(req, customer)
}

pub(crate) fn apply_source_sse_customer_to_copy_object(
    mut req: CopyObjectFluentBuilder,
    customer: Option<SseCustomerHeaders>,
) -> CopyObjectFluentBuilder {
    if let Some(customer) = customer {
        req = req.copy_source_sse_customer_algorithm(customer.algorithm);
        if let (Some(key), Some(key_md5)) = (customer.key, customer.key_md5) {
            req = req
                .copy_source_sse_customer_key(key)
                .copy_source_sse_customer_key_md5(key_md5);
        }
    }
    req
}

pub(crate) fn apply_server_side_encryption_to_put_object(
    mut req: PutObjectFluentBuilder,
    sse: Option<&str>,
    kms_key_id: Option<&str>,
) -> PutObjectFluentBuilder {
    if let Some(alg) = sse {
        req = req.server_side_encryption(ServerSideEncryption::from(alg));
    }
    if let Some(kid) = kms_key_id {
        req = req.ssekms_key_id(kid);
    }
    req
}

pub(crate) fn apply_server_side_encryption_to_create_multipart(
    mut req: CreateMultipartUploadFluentBuilder,
    sse: Option<&str>,
    kms_key_id: Option<&str>,
) -> CreateMultipartUploadFluentBuilder {
    if let Some(alg) = sse {
        req = req.server_side_encryption(ServerSideEncryption::from(alg));
    }
    if let Some(kid) = kms_key_id {
        req = req.ssekms_key_id(kid);
    }
    req
}

pub(crate) fn apply_server_side_encryption_to_copy_object(
    mut req: CopyObjectFluentBuilder,
    sse: Option<&str>,
    kms_key_id: Option<&str>,
) -> CopyObjectFluentBuilder {
    if let Some(alg) = sse {
        req = req.server_side_encryption(ServerSideEncryption::from(alg));
    }
    if let Some(kid) = kms_key_id {
        req = req.ssekms_key_id(kid);
    }
    req
}

/// Parse checksum option into (mode, algorithm) pair.
pub(crate) fn parse_checksum(
    checksum: Option<String>,
) -> Result<(Option<ChecksumMode>, Option<ChecksumAlgorithm>), String> {
    let Some(val) = checksum else {
        return Ok((None, None));
    };
    match val.to_uppercase().as_str() {
        "ENABLED" => Ok((Some(ChecksumMode::Enabled), None)),
        "CRC32" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Crc32))),
        "CRC32C" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Crc32C))),
        "SHA1" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Sha1))),
        "SHA256" => Ok((Some(ChecksumMode::Enabled), Some(ChecksumAlgorithm::Sha256))),
        _ => Err(format!(
            "Invalid checksum value: {}. Use ENABLED, CRC32, CRC32C, SHA1, or SHA256",
            val
        )),
    }
}

/// Compute a base64-encoded checksum of `data` using the requested algorithm.
pub(crate) fn compute_put_checksum(
    data: &[u8],
    checksum_mode: Option<&ChecksumMode>,
    checksum_algorithm: Option<&ChecksumAlgorithm>,
) -> (Option<String>, Option<String>) {
    use sha1::Digest;

    if checksum_mode.is_none() {
        return (None, None);
    }
    let algo = checksum_algorithm.unwrap_or(&ChecksumAlgorithm::Crc32);
    let (name, bytes): (&str, Vec<u8>) = match algo {
        ChecksumAlgorithm::Crc32 => {
            let mut h = crc32fast::Hasher::new();
            h.update(data);
            ("CRC32", h.finalize().to_be_bytes().to_vec())
        }
        ChecksumAlgorithm::Crc32C => {
            let checksum = crc32c::crc32c(data);
            ("CRC32C", checksum.to_be_bytes().to_vec())
        }
        ChecksumAlgorithm::Sha1 => {
            let mut h = sha1::Sha1::new();
            h.update(data);
            ("SHA1", h.finalize().to_vec())
        }
        ChecksumAlgorithm::Sha256 => {
            let mut h = sha2::Sha256::new();
            h.update(data);
            ("SHA256", h.finalize().to_vec())
        }
        _ => return (None, None),
    };
    let encoded = STANDARD.encode(&bytes);
    (Some(name.to_string()), Some(encoded))
}

pub(crate) fn apply_put_checksum_to_put_object(
    mut req: PutObjectFluentBuilder,
    algorithm: Option<&str>,
    value: Option<String>,
) -> PutObjectFluentBuilder {
    match (algorithm, value) {
        (Some("CRC32"), Some(v)) => req = req.checksum_crc32(v),
        (Some("CRC32C"), Some(v)) => req = req.checksum_crc32_c(v),
        (Some("SHA1"), Some(v)) => req = req.checksum_sha1(v),
        (Some("SHA256"), Some(v)) => req = req.checksum_sha256(v),
        _ => {}
    }
    req
}

pub(crate) fn apply_put_checksum_to_upload_part(
    mut req: UploadPartFluentBuilder,
    algorithm: Option<&str>,
    value: Option<&str>,
) -> UploadPartFluentBuilder {
    match (algorithm, value) {
        (Some("CRC32"), Some(v)) => req = req.checksum_crc32(v.to_string()),
        (Some("CRC32C"), Some(v)) => req = req.checksum_crc32_c(v.to_string()),
        (Some("SHA1"), Some(v)) => req = req.checksum_sha1(v.to_string()),
        (Some("SHA256"), Some(v)) => req = req.checksum_sha256(v.to_string()),
        _ => {}
    }
    req
}

pub(crate) fn uploaded_part_checksum(
    response: &UploadPartOutput,
    algorithm: &ChecksumAlgorithm,
    fallback: Option<String>,
) -> Option<String> {
    match algorithm {
        ChecksumAlgorithm::Crc32 => response.checksum_crc32().map(str::to_string).or(fallback),
        ChecksumAlgorithm::Crc32C => response.checksum_crc32_c().map(str::to_string).or(fallback),
        ChecksumAlgorithm::Sha1 => response.checksum_sha1().map(str::to_string).or(fallback),
        ChecksumAlgorithm::Sha256 => response.checksum_sha256().map(str::to_string).or(fallback),
        _ => None,
    }
}

pub(crate) fn completed_part(
    part_number: i32,
    etag: String,
    checksum_enabled: bool,
    algorithm: &ChecksumAlgorithm,
    response: &UploadPartOutput,
    fallback_checksum: Option<String>,
) -> CompletedPart {
    let mut part_builder = CompletedPart::builder()
        .part_number(part_number)
        .e_tag(etag);

    if checksum_enabled {
        if let Some(v) = uploaded_part_checksum(response, algorithm, fallback_checksum) {
            match algorithm {
                ChecksumAlgorithm::Crc32 => part_builder = part_builder.checksum_crc32(v),
                ChecksumAlgorithm::Crc32C => part_builder = part_builder.checksum_crc32_c(v),
                ChecksumAlgorithm::Sha1 => part_builder = part_builder.checksum_sha1(v),
                ChecksumAlgorithm::Sha256 => part_builder = part_builder.checksum_sha256(v),
                _ => {}
            }
        }
    }

    part_builder.build()
}

pub(crate) async fn stream_body_to_writer<W>(
    mut body: ByteStream,
    writer: &mut W,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: AsyncWrite + Unpin,
{
    while let Some(chunk) = body.try_next().await? {
        writer.write_all(&chunk).await?;
    }
    Ok(())
}

/// Largest buffer hsc binds for a single RDMA segment.
///
/// The cuObj client provider serves binds from a pool of registered buffers of
/// this size and rejects anything larger, so every RDMA transfer is split into
/// segments of at most this many bytes.  `main` configures the provider's pool
/// slot size with this value.
#[cfg(feature = "rdma")]
pub(crate) const RDMA_MAX_BIND_SIZE: usize = crate::rdma::DEFAULT_RDMA_BUF_SIZE;

/// RDMA buffers and tokens prepared for one S3 request.
#[cfg(feature = "rdma")]
pub(crate) struct PreparedRdmaTransfer {
    /// One `(channel, handle)` per buffer segment, in data order.
    pub segments: Vec<(Arc<dyn RdmaClientChannel>, RdmaTransferHandle)>,
    /// `|`-joined tokens of all segments, sent as `x-amz-rdma-token`.
    pub token: Vec<u8>,
}

/// Send `request` (a GET for exactly `size` bytes), receiving the data over
/// RDMA when `prepared` is set and the server confirms it, otherwise from the
/// HTTP body, and write it to `writer`.
#[cfg(feature = "rdma")]
async fn send_get_with_optional_rdma_to_writer<W>(
    request: GetObjectFluentBuilder,
    prepared: Option<PreparedRdmaTransfer>,
    size: usize,
    writer: &mut W,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: AsyncWrite + Unpin,
{
    let Some(prepared) = prepared else {
        let response = request.send().await?;
        return stream_body_to_writer(response.body, writer).await;
    };

    // GETs are prepared with a single segment (see `prepare_rdma_get`).
    let channel = Arc::clone(&prepared.segments[0].0);
    let rdma_confirmed = Arc::new(AtomicBool::new(false));
    let interceptor = RdmaInterceptor::new_get(
        prepared.segments,
        prepared.token,
        size,
        Arc::clone(&rdma_confirmed),
        false,
    );
    let response = request.customize().interceptor(interceptor).send().await?;

    if rdma_confirmed.load(Ordering::Acquire) {
        let buf = unsafe { std::slice::from_raw_parts(channel.ptr(), channel.size()) };
        writer.write_all(&buf[..size]).await?;
    } else {
        stream_body_to_writer(response.body, writer).await?;
    }
    Ok(())
}

/// Which bytes of an object an RDMA GET should fetch.
#[cfg(feature = "rdma")]
pub(crate) struct RdmaGetSpan {
    /// First byte to fetch.
    pub start: u64,
    /// Number of bytes to fetch.
    pub len: u64,
    /// The span is the whole object (or the whole of the part selected on the
    /// request).  A whole object that fits in one RDMA segment is fetched
    /// without a `Range` header; anything else is fetched with ranged GETs.
    pub whole_object: bool,
}

/// Fetch `span` of an object over RDMA and write it to `writer`.
///
/// `request` is the base GetObject request (bucket, key, version, SSE-C, …)
/// without a `Range`.  Spans larger than [`RDMA_MAX_BIND_SIZE`] are fetched as
/// consecutive ranged GETs of at most that size, so objects of any size move
/// over RDMA with bounded memory.  A segment whose RDMA buffer cannot be bound,
/// or that the server declines to serve over RDMA, is read from the HTTP body.
#[cfg(feature = "rdma")]
pub(crate) async fn rdma_get_to_writer<W>(
    provider: &Arc<dyn RdmaClientProvider>,
    s3_key: &str,
    request: GetObjectFluentBuilder,
    span: RdmaGetSpan,
    writer: &mut W,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: AsyncWrite + Unpin,
{
    for seg in split_get_span(&span, RDMA_MAX_BIND_SIZE as u64) {
        let size = seg.len as usize;
        let prepared = prepare_rdma_get(provider, s3_key, size);
        let req = if seg.ranged {
            request
                .clone()
                .range(format!("bytes={}-{}", seg.start, seg.start + seg.len - 1))
        } else {
            request.clone()
        };
        send_get_with_optional_rdma_to_writer(req, prepared, size, writer).await?;
    }
    Ok(())
}

/// One GET request of an RDMA download.
#[cfg(feature = "rdma")]
#[derive(Debug, PartialEq, Eq)]
struct GetSegment {
    start: u64,
    len: u64,
    /// Send with a `Range` header (false only for a whole object in one request).
    ranged: bool,
}

/// Split `span` into GET requests of at most `max` bytes each.
#[cfg(feature = "rdma")]
fn split_get_span(span: &RdmaGetSpan, max: u64) -> Vec<GetSegment> {
    if span.whole_object && span.len <= max {
        return vec![GetSegment {
            start: span.start,
            len: span.len,
            ranged: false,
        }];
    }
    let end = span.start + span.len;
    let mut segs = Vec::new();
    let mut off = span.start;
    while off < end {
        let len = max.min(end - off);
        segs.push(GetSegment {
            start: off,
            len,
            ranged: true,
        });
        off += len;
    }
    segs
}

#[cfg(all(test, feature = "rdma"))]
mod tests {
    use super::{split_get_span, GetSegment, RdmaGetSpan};

    fn span(start: u64, len: u64, whole_object: bool) -> RdmaGetSpan {
        RdmaGetSpan {
            start,
            len,
            whole_object,
        }
    }

    fn seg(start: u64, len: u64, ranged: bool) -> GetSegment {
        GetSegment { start, len, ranged }
    }

    #[test]
    fn whole_object_that_fits_is_one_unranged_get() {
        assert_eq!(
            split_get_span(&span(0, 100, true), 100),
            vec![seg(0, 100, false)]
        );
        assert_eq!(
            split_get_span(&span(0, 0, true), 100),
            vec![seg(0, 0, false)]
        );
    }

    #[test]
    fn large_whole_object_is_split_into_ranged_gets() {
        assert_eq!(
            split_get_span(&span(0, 250, true), 100),
            vec![seg(0, 100, true), seg(100, 100, true), seg(200, 50, true)]
        );
    }

    #[test]
    fn ranges_are_always_ranged_and_split_from_their_start() {
        assert_eq!(
            split_get_span(&span(5, 10, false), 100),
            vec![seg(5, 10, true)]
        );
        assert_eq!(
            split_get_span(&span(50, 200, false), 100),
            vec![seg(50, 100, true), seg(150, 100, true)]
        );
    }
}

/// Bind one RDMA buffer of `size` bytes, or `None` (after logging) on failure.
#[cfg(feature = "rdma")]
fn bind_rdma_channel(
    provider: &Arc<dyn RdmaClientProvider>,
    size: usize,
    s3_key: &[u8],
) -> Option<Arc<dyn RdmaClientChannel>> {
    if size == 0 {
        return None;
    }

    match provider.bind(size, s3_key) {
        Ok(ch) => Some(Arc::from(ch)),
        Err(e) => {
            eprintln!("[rdma] bind failed ({e}); falling back to plain HTTP");
            None
        }
    }
}

/// Bind a buffer and prepare a GET token for `size` bytes
/// (at most [`RDMA_MAX_BIND_SIZE`]).  `None` means "use plain HTTP".
#[cfg(feature = "rdma")]
fn prepare_rdma_get(
    provider: &Arc<dyn RdmaClientProvider>,
    s3_key: &str,
    size: usize,
) -> Option<PreparedRdmaTransfer> {
    debug_assert!(size <= RDMA_MAX_BIND_SIZE);
    let channel = bind_rdma_channel(provider, size, s3_key.as_bytes())?;
    match channel.prepare_get(0, size) {
        Ok(h) => Some(PreparedRdmaTransfer {
            token: h.token().to_vec(),
            segments: vec![(channel, h)],
        }),
        Err(e) => {
            eprintln!("[rdma] prepare_get failed ({e}); falling back to plain HTTP");
            None
        }
    }
}

/// Whether a PUT body of `len` bytes fits in one RDMA buffer; logs why not.
#[cfg(feature = "rdma")]
pub(crate) fn rdma_put_fits(len: usize, failure_context: &str) -> bool {
    if len <= RDMA_MAX_BIND_SIZE {
        return true;
    }
    eprintln!(
        "[rdma] request body {failure_context}({len} bytes) exceeds one RDMA buffer \
         ({RDMA_MAX_BIND_SIZE} bytes); sending over plain HTTP \
         (use multipart with --part-size of at most {}m for RDMA)",
        RDMA_MAX_BIND_SIZE >> 20
    );
    false
}

/// Copy `data` into an RDMA buffer and prepare a PUT token, for a single
/// PutObject/UploadPart.  `None` (after logging) means "send the body over
/// plain HTTP".
///
/// One request can carry only one RDMA buffer: HyperStore rejects `|`-joined
/// multi-token PUTs.  Bodies larger than [`RDMA_MAX_BIND_SIZE`] therefore go
/// over HTTP; multipart uploads with parts of at most that size use RDMA.
#[cfg(feature = "rdma")]
pub(crate) fn prepare_rdma_put(
    provider: &Arc<dyn RdmaClientProvider>,
    s3_key: &str,
    data: &[u8],
    failure_context: &str,
) -> Option<PreparedRdmaTransfer> {
    if !rdma_put_fits(data.len(), failure_context) {
        return None;
    }
    let channel = bind_rdma_channel(provider, data.len(), s3_key.as_bytes())?;
    let buf = unsafe { std::slice::from_raw_parts_mut(channel.ptr(), channel.size()) };
    buf[..data.len()].copy_from_slice(data);
    match channel.prepare_put(0, data.len()) {
        Ok(h) => Some(PreparedRdmaTransfer {
            token: h.token().to_vec(),
            segments: vec![(channel, h)],
        }),
        Err(e) => {
            eprintln!(
                "[rdma] prepare_put failed {failure_context}({e}); falling back to plain HTTP"
            );
            None
        }
    }
}
