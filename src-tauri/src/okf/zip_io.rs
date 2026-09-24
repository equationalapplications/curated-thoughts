//! Bundle file I/O. Zip reads enforce the §8 untrusted-archive advice:
//! entry-count cap and a running total of *actual* decompressed bytes
//! (declared sizes are attacker-controlled).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::okf::types::OkfFile;

pub const MAX_ZIP_ENTRIES: usize = 10_000;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

pub fn read_bundle_source(path: &Path) -> Result<Vec<OkfFile>> {
    if path.is_dir() {
        read_bundle_dir(path)
    } else {
        read_bundle_zip(path)
    }
}

fn read_bundle_dir(root: &Path) -> Result<Vec<OkfFile>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "md") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .context("walkdir escaped root")?
            .to_string_lossy()
            .replace('\\', "/");
        files.push(OkfFile {
            path: rel,
            content: std::fs::read_to_string(entry.path())
                .with_context(|| format!("reading {}", entry.path().display()))?,
        });
    }
    Ok(files)
}

fn read_bundle_zip(path: &Path) -> Result<Vec<OkfFile>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("not a valid zip archive")?;
    if archive.len() > MAX_ZIP_ENTRIES {
        bail!(
            "bundle rejected: {} entries exceeds cap {MAX_ZIP_ENTRIES}",
            archive.len()
        );
    }
    let mut files = Vec::new();
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if entry.is_dir() || !entry.name().ends_with(".md") {
            continue;
        }
        let name = entry.name().to_string();
        let mut content = String::new();
        let budget = MAX_TOTAL_BYTES.saturating_sub(total) + 1;
        let mut reader = entry.take(budget);
        let read = reader
            .read_to_string(&mut content)
            .with_context(|| format!("reading {name}"))? as u64;
        total += read;
        if total > MAX_TOTAL_BYTES {
            bail!("bundle rejected: decompressed size exceeds {MAX_TOTAL_BYTES} bytes");
        }
        files.push(OkfFile {
            path: name,
            content,
        });
    }
    Ok(files)
}

pub fn write_bundle_zip(dest: &Path, files: &[OkfFile]) -> Result<()> {
    let file = File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    write_bundle_zip_into(file, files).with_context(|| format!("writing {}", dest.display()))
}

/// Write the bundle to an already-open handle. The exporter uses this so the
/// exclusive 0o600 temp file it created is written through the held handle —
/// the bundle bytes are never written to a path that could be swapped.
pub fn write_bundle_zip_into<W: std::io::Write + std::io::Seek + 'static>(
    file: W,
    files: &[OkfFile],
) -> Result<()> {
    let mut writer = zip::ZipWriter::new(file);
    // Fixed mtime (2026-01-01 00:00:00 UTC) so identical content produces
    // byte-identical archives — required for nightly-backup change detection.
    let fixed = zip::DateTime::from_date_and_time(2026, 1, 1, 0, 0, 0).unwrap_or_default();
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(fixed);
    for f in files {
        writer.start_file(&f.path, options)?;
        writer.write_all(f.content.as_bytes())?;
    }
    // zip 8.6: `finish()` returns Result<W, ZipWriterResult> handing back the
    // inner writer W. Syncing requires a real File, so the generic writer is
    // downcast via its Any impl when it is one (the exporter and this fn's
    // path-based wrapper both pass std::fs::File). Non-File writers skip fsync.
    let mut file = writer
        .finish()
        .with_context(|| "finishing bundle".to_string())?;
    file.flush().context("flushing bundle")?;
    {
        use std::any::Any;
        let any: &mut dyn Any = &mut file;
        if let Some(f) = any.downcast_mut::<std::fs::File>() {
            f.sync_all().context("syncing bundle")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::okf::types::OkfFile;

    #[test]
    fn zip_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("bundle.zip");
        let files = vec![
            OkfFile {
                path: "index.md".into(),
                content: "---\nokf_version: 0.1\n---\n".into(),
            },
            OkfFile {
                path: "entities/demo/index.md".into(),
                content: "[Event log](./log.md)\n".into(),
            },
        ];
        write_bundle_zip(&zip_path, &files).unwrap();
        let mut read = read_bundle_source(&zip_path).unwrap();
        read.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].path, "index.md");
    }

    #[test]
    fn directory_source_reads_md_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("entities/demo")).unwrap();
        std::fs::write(dir.path().join("index.md"), "---\nokf_version: 0.1\n---\n").unwrap();
        std::fs::write(dir.path().join("entities/demo/log.md"), "").unwrap();
        let files = read_bundle_source(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn zip_bytes_deterministic() {
        let files = vec![OkfFile {
            path: "index.md".into(),
            content: "---\nokf_version: 0.2\n---\n".into(),
        }];
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.zip");
        let b = dir.path().join("b.zip");
        write_bundle_zip(&a, &files).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_bundle_zip(&b, &files).unwrap();
        let ha = sha256_hex_file(&a).unwrap();
        let hb = sha256_hex_file(&b).unwrap();
        assert_eq!(ha, hb, "identical inputs must produce identical zip bytes");
    }

    // 64KiB-chunked sha256 helper local to the tests module
    fn sha256_hex_file(path: &std::path::Path) -> anyhow::Result<String> {
        use sha2::{Digest, Sha256};
        use std::io::Read as _;
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }

    #[test]
    fn rejects_zip_with_too_many_entries() {
        let brain_tmp = tempfile::TempDir::new().unwrap();
        let brain = brain_tmp.path().to_string_lossy().into_owned();
        // Redirect the brain dir: this test resolved the LIVE ~/.brain
        // without a guard (issue #178).
        temp_env::with_vars(
            [
                ("CURATED_BRAIN_DIR", Some(brain.as_str())),
                // Explicitly clear CONFIG: an inherited value would override the
                // redirected brain dir and make the test non-hermetic (#178).
                ("CURATED_BRAIN_CONFIG", None::<&str>),
                ("CURATED_BRAIN_DB", None::<&str>),
            ],
            || {
                let dir = tempfile::tempdir().unwrap();
                let zip_path = dir.path().join("bomb.zip");
                let many: Vec<OkfFile> = (0..=MAX_ZIP_ENTRIES)
                    .map(|i| OkfFile {
                        path: format!("entities/e/facts/f{i}.md"),
                        content: "x".into(),
                    })
                    .collect();
                write_bundle_zip(&zip_path, &many).unwrap();
                assert!(read_bundle_source(&zip_path).is_err());
            },
        );
    }
}
