//! Where bundles go: any presigned URL, or an Inside task.

pub mod inside;

use anyhow::{bail, Result};

/// PUT the archive to a presigned URL.
pub fn put(url: &str, bytes: &[u8]) -> Result<String> {
    match ureq::put(url)
        .set("Content-Type", "application/zip")
        .send_bytes(bytes)
    {
        Ok(r) => Ok(r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            bail!(
                "upload refused (HTTP {code}): {}",
                body.chars().take(2000).collect::<String>()
            )
        }
        Err(e) => bail!("upload failed: {e}"),
    }
}

pub fn get(url: &str) -> Result<Vec<u8>> {
    let r = ureq::get(url).call()?;
    let mut buf = Vec::new();
    std::io::Read::read_to_end(
        &mut r
            .into_reader()
            .take(crate::bundle::MAX_ARCHIVE_BYTES as u64 + 1),
        &mut buf,
    )?;
    Ok(buf)
}

use std::io::Read as _;
