//! Download and extract archives.
//!
//! Each [`Download`] is fetched via HTTP(S) and optionally extracted into a
//! destination directory. A `.done` marker file is written on success so
//! subsequent runs skip the download.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::types::Download;
use crate::ui::UiConfig;

/// Run a single download step. Skips if `dest/.done` already exists.
/// Returns `true` if a download was performed, `false` if skipped.
pub fn run_download(dl: &Download, ui: &UiConfig, quiet: bool) -> Result<bool> {
    let done_marker = Path::new(&dl.dest).join(".done");
    if done_marker.exists() {
        return Ok(false);
    }

    if !quiet {
        ui.print_download(&dl.url, &dl.dest);
    }

    fs::create_dir_all(&dl.dest)
        .with_context(|| format!("failed to create directory: {}", dl.dest))?;

    let format = dl
        .extract
        .as_deref()
        .unwrap_or_else(|| infer_format(&dl.url));

    let response = ureq::get(&dl.url)
        .call()
        .with_context(|| format!("failed to download: {}", dl.url))?;

    let reader = response.into_body().into_reader();

    match format {
        "tar.gz" | "tgz" => extract_tar_gz(reader, &dl.dest, dl.strip)?,
        "tar.xz" | "txz" => {
            bail!("tar.xz extraction not yet supported — use tar.gz");
        }
        "tar.bz2" | "tbz2" => {
            bail!("tar.bz2 extraction not yet supported — use tar.gz");
        }
        "tar" => extract_tar(reader, &dl.dest, dl.strip)?,
        "none" => {
            // Save raw file — use the URL's filename.
            let filename = dl.url.rsplit('/').next().unwrap_or("download");
            let out_path = Path::new(&dl.dest).join(filename);
            let mut file = fs::File::create(&out_path)
                .with_context(|| format!("failed to create {}", out_path.display()))?;
            let mut buf_reader = std::io::BufReader::new(reader);
            std::io::copy(&mut buf_reader, &mut file)
                .with_context(|| format!("failed to write {}", out_path.display()))?;
        }
        other => bail!("unsupported archive format: {other}"),
    }

    // Write the done marker.
    fs::write(&done_marker, "")
        .with_context(|| format!("failed to write done marker: {}", done_marker.display()))?;

    Ok(true)
}

/// Extract a gzipped tar archive, stripping `strip` leading path components.
fn extract_tar_gz(reader: impl Read, dest: &str, strip: u32) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(reader);
    extract_tar(gz, dest, strip)
}

/// Extract a tar archive, stripping `strip` leading path components.
fn extract_tar(reader: impl Read, dest: &str, strip: u32) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    let dest_path = Path::new(dest);

    for entry in archive.entries().context("failed to read tar entries")? {
        let mut entry = entry.context("failed to read tar entry")?;
        let orig_path = entry
            .path()
            .context("invalid path in tar entry")?
            .into_owned();

        // Strip leading components.
        let components: Vec<_> = orig_path.components().collect();
        if components.len() <= strip as usize {
            continue; // Path is entirely stripped away.
        }
        let stripped: std::path::PathBuf = components[strip as usize..].iter().collect();
        let out_path = dest_path.join(&stripped);

        // Create parent directories.
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create dir: {}", parent.display()))?;
        }

        // Only extract regular files (skip directories, symlinks, etc.).
        if entry.header().entry_type().is_file() {
            entry
                .unpack(&out_path)
                .with_context(|| format!("failed to extract: {}", out_path.display()))?;
        }
    }
    Ok(())
}

/// Infer archive format from a URL's file extension.
fn infer_format(url: &str) -> &'static str {
    // Strip query string / fragment.
    let path = url.split('?').next().unwrap_or(url);
    let path = path.split('#').next().unwrap_or(path);

    if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
        "tar.gz"
    } else if path.ends_with(".tar.xz") || path.ends_with(".txz") {
        "tar.xz"
    } else if path.ends_with(".tar.bz2") || path.ends_with(".tbz2") {
        "tar.bz2"
    } else if path.ends_with(".tar") {
        "tar"
    } else if path.ends_with(".zip") {
        "zip"
    } else {
        "none"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_format_from_url() {
        assert_eq!(infer_format("https://example.com/foo.tar.gz"), "tar.gz");
        assert_eq!(infer_format("https://example.com/foo.tgz"), "tar.gz");
        assert_eq!(infer_format("https://example.com/foo.tar"), "tar");
        assert_eq!(infer_format("https://example.com/foo.zip"), "zip");
        assert_eq!(infer_format("https://example.com/foo.bin"), "none");
        assert_eq!(
            infer_format("https://example.com/foo.tar.gz?token=abc"),
            "tar.gz"
        );
    }
}
