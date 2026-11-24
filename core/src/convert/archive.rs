//! Transactional ZIP / MCWORLD export. World conversion always writes a real
//! directory first; archive publication only occurs after all writers close.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::{ConversionOptions, ConversionPlan, Edition, Progress};
use crate::error::{Result, WorldPortError, io_result};
use crate::report::ConversionReport;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Output {
    Directory,
    Zip,
    Mcworld,
}

impl Output {
    pub(super) fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("zip") => Self::Zip,
            Some(ext) if ext.eq_ignore_ascii_case("mcworld") => Self::Mcworld,
            _ => Self::Directory,
        }
    }
}

pub(super) fn convert(
    plan: &ConversionPlan,
    options: &ConversionOptions,
    source: Edition,
    target: Edition,
) -> Result<ConversionReport> {
    let destination = &plan.target;
    if Output::from_path(destination) == Output::Mcworld && target != Edition::Bedrock {
        return Err(WorldPortError::invalid("MCWORLD output requires a Bedrock target"));
    }
    // Never replace an existing archive, including a dangling symlink.
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(WorldPortError::invalid(format!(
            "{} already exists; choose a new output file", destination.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(WorldPortError::io(destination, error)),
    }
    let parent = destination.parent().filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Refuse a staging directory inside the source world, even via a symlink.
    let source_dir = io_result(&plan.source, fs::canonicalize(&plan.source))?;
    let mut ancestor = parent;
    while !ancestor.exists() {
        ancestor = ancestor.parent().filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
    }
    if io_result(ancestor, fs::canonicalize(ancestor))?.starts_with(&source_dir) {
        return Err(WorldPortError::invalid("archive output must be outside the source world"));
    }
    io_result(parent, fs::create_dir_all(parent))?;
    let parent = io_result(parent, fs::canonicalize(parent))?;
    if parent.starts_with(&source_dir) {
        return Err(WorldPortError::invalid("archive output must be outside the source world"));
    }
    let staging = io_result(&parent, tempfile::Builder::new().prefix(".worldport-").tempdir_in(&parent))?;
    let mut staged_plan = plan.clone();
    staged_plan.target = staging.path().join("world");
    staged_plan.overwrite = false;
    let mut report = super::run_directory(&staged_plan, options, source, target)?;
    options.check_cancelled()?;
    export(&staged_plan.target, destination, &parent, options)?;
    report.target_path = destination.display().to_string();
    Ok(report)
}

fn files(root: &Path, options: &ConversionOptions) -> Result<Vec<(PathBuf, u64, bool)>> {
    let mut result = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        options.check_cancelled()?;
        for entry in io_result(&dir, fs::read_dir(&dir))? {
            options.check_cancelled()?;
            let entry = io_result(&dir, entry)?;
            let path = entry.path();
            let kind = io_result(&path, entry.file_type())?;
            if kind.is_dir() {
                pending.push(path.clone());
                result.push((path, 0, true));
            } else if kind.is_file() {
                let size = io_result(&path, entry.metadata())?.len();
                result.push((path, size, false));
            } else {
                return Err(WorldPortError::invalid(format!("unsupported archive entry {}", path.display())));
            }
        }
    }
    result.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

fn zip_error(error: zip::result::ZipError) -> WorldPortError {
    WorldPortError::other(format!("ZIP export: {error}"))
}

fn export(root: &Path, destination: &Path, parent: &Path, options: &ConversionOptions) -> Result<()> {
    let entries = files(root, options)?;
    let total = entries.iter().fold(0_u64, |sum, (_, size, _)| sum.saturating_add(*size));
    let mut done = 0_u64;
    options.report(Progress::new("archive", done, Some(total)));
    let mut temporary = io_result(parent, tempfile::Builder::new().prefix(".worldport-archive-").tempfile_in(parent))?;
    {
        let mut zip = ZipWriter::new(temporary.as_file_mut());
        let mut buffer = vec![0_u8; 256 * 1024];
        for (path, size, directory) in entries {
            options.check_cancelled()?;
            let relative = path.strip_prefix(root).map_err(|error| WorldPortError::other(error.to_string()))?;
            let mut parts = Vec::new();
            for component in relative.components() {
                let std::path::Component::Normal(name) = component else {
                    return Err(WorldPortError::invalid("invalid archive entry path"));
                };
                let name = name.to_str().filter(|name| !name.contains(['\\', ':']))
                    .ok_or_else(|| WorldPortError::invalid("invalid archive entry name"))?;
                parts.push(name);
            }
            let name = parts.join("/");
            let already_compressed = path.extension().and_then(|ext| ext.to_str())
                .is_some_and(|ext| ["mca", "ldb", "sst", "png", "jpg", "jpeg", "gz", "zip"].iter().any(|suffix| ext.eq_ignore_ascii_case(suffix)));
            let method = if directory || already_compressed { CompressionMethod::Stored } else { CompressionMethod::Deflated };
            let settings = SimpleFileOptions::default().compression_method(method)
                .large_file(size >= 0x8000_0000);
            if directory {
                zip.add_directory(format!("{name}/"), settings).map_err(zip_error)?;
                continue;
            }
            zip.start_file(name, settings).map_err(zip_error)?;
            let mut input = io_result(&path, File::open(&path))?;
            loop {
                options.check_cancelled()?;
                let count = io_result(&path, input.read(&mut buffer))?;
                if count == 0 { break; }
                io_result(destination, zip.write_all(&buffer[..count]))?;
                done = done.saturating_add(count as u64);
                options.report(Progress::new("archive", done, Some(total)));
            }
        }
        zip.finish().map_err(zip_error)?;
    }
    io_result(destination, temporary.as_file().sync_all())?;
    options.check_cancelled()?;
    // Same-volume atomic/no-clobber publication; dropping the temp file on
    // error or cancellation leaves no partial final archive.
    temporary.persist_noclobber(destination)
        .map_err(|error| WorldPortError::io(destination, error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_root_and_no_clobber() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("world");
        fs::create_dir_all(root.join("db")).unwrap();
        fs::write(root.join("level.dat"), b"metadata").unwrap();
        fs::write(root.join("db/000001.ldb"), b"database").unwrap();
        let destination = temp.path().join("world.mcworld");
        export(&root, &destination, temp.path(), &ConversionOptions::default()).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(&destination).unwrap()).unwrap();
        let mut content = Vec::new();
        zip.by_name("level.dat").unwrap().read_to_end(&mut content).unwrap();
        assert_eq!(content, b"metadata");
        assert!(zip.by_name("db/000001.ldb").is_ok());
        drop(zip);
        let original = fs::read(&destination).unwrap();
        assert!(export(&root, &destination, temp.path(), &ConversionOptions::default()).is_err());
        assert_eq!(fs::read(&destination).unwrap(), original);
    }

    #[test]
    fn cancelled_archive_does_not_publish() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("world");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("level.dat"), b"data").unwrap();
        let destination = temp.path().join("out.zip");
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = flag.clone();
        let options = ConversionOptions::with_cancel(flag).with_progress(std::sync::Arc::new(move |progress| {
            if progress.done > 0 { cancel.store(true, std::sync::atomic::Ordering::Relaxed); }
        }));
        assert!(matches!(export(&root, &destination, temp.path(), &options), Err(WorldPortError::Cancelled)));
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
