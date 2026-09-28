//! Explicit process-local CLI overrides. Bundle contents cannot enable these.
use std::sync::OnceLock;

#[derive(clap::Args, Clone, Copy, Debug, Default)]
pub struct Overrides {
    /// Allow traversal, absolute destinations and filesystem symlinks
    #[arg(long, global = true)]
    pub skip_path_checks: bool,
    /// Disable Flightlog archive, entry and input size limits
    #[arg(long, global = true)]
    pub skip_size_checks: bool,
    /// Ignore manifest SHA-256 mismatches (ZIP decoding must still succeed)
    #[arg(long, global = true)]
    pub skip_checksums: bool,
    /// Skip bundle schema and file-inventory validation
    #[arg(long, global = true)]
    pub skip_format_checks: bool,
    /// Skip input redaction and opaque-content checks, regardless of manifest mode
    #[arg(long, global = true)]
    pub skip_content_checks: bool,
    /// Replace existing outputs and restore files; merge extraction directories
    #[arg(long, global = true)]
    pub overwrite: bool,
    /// Permit plaintext HTTP transfers
    #[arg(long, global = true)]
    pub allow_http: bool,
}

static OVERRIDES: OnceLock<Overrides> = OnceLock::new();
pub fn init(mut options: Overrides, yolo: bool) {
    if yolo {
        options = Overrides {
            skip_path_checks: true,
            skip_size_checks: true,
            skip_checksums: true,
            skip_format_checks: true,
            skip_content_checks: true,
            overwrite: true,
            allow_http: true,
        };
    }
    OVERRIDES.set(options).expect("CLI initialized once");
}
pub fn get() -> Overrides {
    OVERRIDES.get().copied().unwrap_or_default()
}
pub fn limit(normal: u64) -> u64 {
    if get().skip_size_checks {
        u64::MAX
    } else {
        normal
    }
}
