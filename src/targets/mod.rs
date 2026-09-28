//! HTTPS-only transfer; signed URLs are never included in diagnostics.
use anyhow::{bail, Context, Result};
use std::{io::Read, path::PathBuf, time::Duration};
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .https_only(true)
        .redirects(0)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .build()
}
fn check(url: &str) -> Result<()> {
    if !url.starts_with("https://") || url.contains(['\r', '\n']) {
        bail!("transfer requires an HTTPS URL");
    }
    Ok(())
}
pub fn url_input(url: Option<String>, file: Option<PathBuf>) -> Result<String> {
    let text = match (url, file) {
        (Some(u), None) => u,
        (None, Some(p)) if p == std::path::Path::new("-") => {
            let mut s = String::new();
            std::io::stdin().take(16_385).read_to_string(&mut s)?;
            s
        }
        (None, Some(p)) => String::from_utf8(crate::safe_fs::read(&p, 16_384)?)
            .context("URL file must be UTF-8")?,
        _ => bail!("supply exactly one URL or --url-file"),
    };
    if text.len() > 16_384 {
        bail!("URL exceeds size limit");
    }
    let text = text.trim().to_string();
    check(&text)?;
    Ok(text)
}
pub fn put(url: &str, bytes: &[u8]) -> Result<()> {
    check(url)?;
    match agent()
        .put(url)
        .set("Content-Type", "application/zip")
        .send_bytes(bytes)
    {
        Ok(r) if (200..300).contains(&r.status()) => Ok(()),
        Ok(_) => bail!("upload refused: redirects are disabled"),
        Err(ureq::Error::Status(code, _)) => bail!("upload refused (HTTP {code})"),
        Err(_) => bail!("upload transport failed (URL omitted)"),
    }
}
pub fn get(url: &str) -> Result<Vec<u8>> {
    check(url)?;
    let r = agent()
        .get(url)
        .call()
        .map_err(|_| anyhow::anyhow!("download failed (URL omitted)"))?;
    if !(200..300).contains(&r.status()) {
        bail!("download refused: redirects are disabled");
    }
    let mut bytes = Vec::new();
    r.into_reader()
        .take(crate::bundle::MAX_ARCHIVE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("download interrupted"))?;
    if bytes.len() > crate::bundle::MAX_ARCHIVE_BYTES {
        bail!("download exceeds size limit");
    }
    Ok(bytes)
}
