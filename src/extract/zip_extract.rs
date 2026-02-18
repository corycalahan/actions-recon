use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, info};

/// Maximum size of a single extracted file (100 MB).
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Maximum number of files allowed in a single ZIP.
const MAX_FILE_COUNT: usize = 1000;

/// Maximum total extracted size (500 MB).
const MAX_TOTAL_SIZE: u64 = 500 * 1024 * 1024;

/// Errors that can occur during ZIP extraction.
#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("ZIP archive is invalid or corrupt: {0}")]
    InvalidZip(#[from] zip::result::ZipError),

    #[error("I/O error during extraction: {0}")]
    Io(#[from] io::Error),

    #[error("Path traversal detected: entry \"{0}\" escapes the output directory")]
    PathTraversal(String),

    #[error("Archive contains a symlink: \"{0}\" — symlinks are not allowed")]
    SymlinkRejected(String),

    #[error("File \"{name}\" exceeds the maximum size of {max_mb} MB (actual: {actual_mb:.1} MB)")]
    FileTooLarge {
        name: String,
        max_mb: u64,
        actual_mb: f64,
    },

    #[error("Archive contains too many files ({count}); maximum is {max}")]
    TooManyFiles { count: usize, max: usize },

    #[error("Total extracted size ({total_mb:.1} MB) exceeds the limit of {max_mb} MB")]
    TotalSizeTooLarge { total_mb: f64, max_mb: u64 },
}

/// Result of a successful extraction.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ExtractResult {
    /// Directory where files were extracted.
    pub output_dir: PathBuf,
    /// Number of files extracted.
    pub file_count: usize,
    /// Total bytes written.
    pub total_bytes: u64,
}

/// Safely extract a ZIP archive into `output_dir`.
///
/// Validates against:
/// - Path traversal (Zip Slip)
/// - Symlinks
/// - Oversized individual files (100 MB)
/// - Excessive file count (1000)
/// - Excessive total size (500 MB)
pub fn extract_zip(zip_bytes: &[u8], output_dir: &Path) -> Result<ExtractResult, ExtractError> {
    let cursor = io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)?;

    let entry_count = archive.len();
    if entry_count > MAX_FILE_COUNT {
        return Err(ExtractError::TooManyFiles {
            count: entry_count,
            max: MAX_FILE_COUNT,
        });
    }

    // Canonicalize (or create then canonicalize) the output directory
    fs::create_dir_all(output_dir)?;
    let canonical_output = output_dir.canonicalize()?;

    let mut total_bytes: u64 = 0;
    let mut file_count: usize = 0;

    for i in 0..entry_count {
        let mut entry = archive.by_index(i)?;
        let entry_name = entry.name().to_string();

        // Reject symlinks
        if entry.is_symlink() {
            return Err(ExtractError::SymlinkRejected(entry_name));
        }

        // Build the target path and validate against traversal
        let target = canonical_output.join(&entry_name);

        // Canonicalize parent to catch /../ tricks — the parent must exist
        // for files, so we create directories first for directory entries.
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            let canonical_target = target.canonicalize()?;
            if !canonical_target.starts_with(&canonical_output) {
                return Err(ExtractError::PathTraversal(entry_name));
            }
            debug!(entry = %entry_name, "Created directory");
            continue;
        }

        // For files: validate size before extracting
        let uncompressed = entry.size();
        if uncompressed > MAX_FILE_SIZE {
            return Err(ExtractError::FileTooLarge {
                name: entry_name,
                max_mb: MAX_FILE_SIZE / (1024 * 1024),
                actual_mb: uncompressed as f64 / (1024.0 * 1024.0),
            });
        }

        // Check total size budget
        if total_bytes + uncompressed > MAX_TOTAL_SIZE {
            return Err(ExtractError::TotalSizeTooLarge {
                total_mb: (total_bytes + uncompressed) as f64 / (1024.0 * 1024.0),
                max_mb: MAX_TOTAL_SIZE / (1024 * 1024),
            });
        }

        // Ensure parent directory exists
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        // Validate path traversal for the file
        // We need to check the parent since the file doesn't exist yet
        if let Some(parent) = target.parent() {
            let canonical_parent = parent.canonicalize()?;
            if !canonical_parent.starts_with(&canonical_output) {
                return Err(ExtractError::PathTraversal(entry_name));
            }
        }

        // Extract with a capped read to prevent zip bombs
        let mut outfile = fs::File::create(&target)?;
        let mut limited = (&mut entry).take(MAX_FILE_SIZE + 1);
        let bytes_written = io::copy(&mut limited, &mut outfile)?;

        if bytes_written > MAX_FILE_SIZE {
            // Clean up the oversized file
            let _ = fs::remove_file(&target);
            return Err(ExtractError::FileTooLarge {
                name: entry_name,
                max_mb: MAX_FILE_SIZE / (1024 * 1024),
                actual_mb: bytes_written as f64 / (1024.0 * 1024.0),
            });
        }

        total_bytes += bytes_written;
        file_count += 1;

        debug!(entry = %entry_name, bytes = bytes_written, "Extracted file");
    }

    info!(
        dir = %canonical_output.display(),
        files = file_count,
        total_bytes,
        "ZIP extraction complete"
    );

    Ok(ExtractResult {
        output_dir: canonical_output.clone(),
        file_count,
        total_bytes,
    })
}

/// After extracting the top-level ZIP, find and extract any nested ZIPs
/// (e.g. `runner-diagnostic-logs/*.zip`) in-place, replacing the `.zip` file
/// with a directory of the same name (minus extension).
///
/// This is best-effort — failures are logged but don't fail the overall extraction.
pub fn extract_nested_zips(base_dir: &Path) {
    let walker = match fs::read_dir(base_dir) {
        Ok(w) => w,
        Err(_) => return,
    };

    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Recurse into subdirectories
            extract_nested_zips_in_dir(&path);
        }
    }
}

/// Scan a specific subdirectory for `.zip` files and extract them.
fn extract_nested_zips_in_dir(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "zip") {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("extracted");
            let extract_dir = dir.join(stem);

            match fs::read(&path) {
                Ok(zip_bytes) => {
                    match extract_zip(&zip_bytes, &extract_dir) {
                        Ok(result) => {
                            info!(
                                nested_zip = %path.display(),
                                files = result.file_count,
                                "Extracted nested ZIP"
                            );
                            // Remove the original .zip after successful extraction
                            let _ = fs::remove_file(&path);
                        }
                        Err(e) => {
                            tracing::warn!(
                                nested_zip = %path.display(),
                                error = %e,
                                "Failed to extract nested ZIP — skipping"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        nested_zip = %path.display(),
                        error = %e,
                        "Failed to read nested ZIP — skipping"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: create a valid ZIP in memory with the given entries.
    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn test_extract_simple_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_data = make_zip(&[("hello.txt", b"Hello, world!")]);
        let result = extract_zip(&zip_data, tmp.path()).unwrap();

        assert_eq!(result.file_count, 1);
        assert_eq!(result.total_bytes, 13);

        let content = fs::read_to_string(tmp.path().join("hello.txt")).unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[test]
    fn test_extract_nested_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_data = make_zip(&[("a/b/deep.log", b"nested content")]);
        let result = extract_zip(&zip_data, tmp.path()).unwrap();

        assert_eq!(result.file_count, 1);
        assert!(tmp.path().join("a/b/deep.log").exists());
    }

    #[test]
    fn test_reject_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_data = make_zip(&[("../escape.txt", b"bad")]);
        let err = extract_zip(&zip_data, tmp.path()).unwrap_err();

        assert!(matches!(err, ExtractError::PathTraversal(_)));
    }

    #[test]
    fn test_reject_too_many_files() {
        let tmp = tempfile::tempdir().unwrap();
        let entries: Vec<(String, Vec<u8>)> = (0..1001)
            .map(|i| (format!("file_{i}.txt"), b"x".to_vec()))
            .collect();
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        let zip_data = make_zip(&refs);
        let err = extract_zip(&zip_data, tmp.path()).unwrap_err();

        assert!(matches!(err, ExtractError::TooManyFiles { .. }));
    }

    #[test]
    fn test_multiple_files() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_data = make_zip(&[
            ("log1.txt", b"first log"),
            ("log2.txt", b"second log"),
            ("subdir/log3.txt", b"third log"),
        ]);
        let result = extract_zip(&zip_data, tmp.path()).unwrap();

        assert_eq!(result.file_count, 3);
        assert!(tmp.path().join("log1.txt").exists());
        assert!(tmp.path().join("log2.txt").exists());
        assert!(tmp.path().join("subdir/log3.txt").exists());
    }
}
