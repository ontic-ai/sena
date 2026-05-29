use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::MakeWriter;

const CLI_LOG_FILE_NAME: &str = "sena-cli.log";
const CLI_LOG_ARCHIVE_NAME: &str = "sena-cli.log.1";
const CLI_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub fn init_tracing() -> io::Result<PathBuf> {
    let log_path = match prepare_log_file_path(resolve_primary_log_path()?) {
        Ok(path) => path,
        Err(primary_error) => {
            let fallback_path = fallback_log_path();
            prepare_log_file_path(fallback_path).map_err(|fallback_error| {
                io::Error::new(
                    fallback_error.kind(),
                    format!(
                        "failed to initialize primary CLI log path ({primary_error}); fallback also failed ({fallback_error})"
                    ),
                )
            })?
        }
    };

    let writer = RollingFileMakeWriter::new(log_path.clone());
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .with_writer(writer)
        .try_init()
        .map_err(|error| io::Error::other(format!("failed to initialize tracing: {error}")))?;

    Ok(log_path)
}

fn resolve_primary_log_path() -> io::Result<PathBuf> {
    Ok(resolve_sena_dir()?.join("logs").join(CLI_LOG_FILE_NAME))
}

fn resolve_sena_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let Some(appdata) = std::env::var_os("APPDATA") else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "APPDATA is not set",
            ));
        };

        Ok(PathBuf::from(appdata).join("sena"))
    }

    #[cfg(target_os = "macos")]
    {
        let Some(home) = std::env::var_os("HOME") else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "HOME is not set"));
        };

        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("sena"))
    }

    #[cfg(target_os = "linux")]
    {
        let Some(home) = std::env::var_os("HOME") else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "HOME is not set"));
        };

        Ok(PathBuf::from(home).join(".config").join("sena"))
    }
}

fn fallback_log_path() -> PathBuf {
    std::env::temp_dir().join(CLI_LOG_FILE_NAME)
}

fn prepare_log_file_path(path: PathBuf) -> io::Result<PathBuf> {
    ensure_parent_dir(&path)?;
    rotate_log_file_if_needed(&path)?;
    Ok(path)
}

fn ensure_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    Ok(())
}

fn rotate_log_file_if_needed(path: &Path) -> io::Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };

    if metadata.len() <= CLI_LOG_MAX_BYTES {
        return Ok(());
    }

    let archive_path = path.with_file_name(CLI_LOG_ARCHIVE_NAME);
    if archive_path.exists() {
        fs::remove_file(&archive_path)?;
    }

    fs::rename(path, archive_path)?;
    Ok(())
}

fn open_log_file(path: &Path) -> io::Result<File> {
    ensure_parent_dir(path)?;
    rotate_log_file_if_needed(path)?;
    OpenOptions::new().create(true).append(true).open(path)
}

struct RollingFileMakeWriter {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl RollingFileMakeWriter {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        match self.write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

enum RollingWriterInner {
    File(File),
    Sink(io::Sink),
}

impl Write for RollingWriterInner {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::File(file) => file.write(buf),
            Self::Sink(sink) => sink.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::File(file) => file.flush(),
            Self::Sink(sink) => sink.flush(),
        }
    }
}

struct RollingFileWriter<'a> {
    _guard: MutexGuard<'a, ()>,
    writer: RollingWriterInner,
}

impl Write for RollingFileWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<'a> MakeWriter<'a> for RollingFileMakeWriter {
    type Writer = RollingFileWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        let guard = self.lock();
        let writer = open_log_file(&self.path)
            .map(RollingWriterInner::File)
            .unwrap_or_else(|_| RollingWriterInner::Sink(io::sink()));

        RollingFileWriter {
            _guard: guard,
            writer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CLI_LOG_ARCHIVE_NAME, CLI_LOG_FILE_NAME, CLI_LOG_MAX_BYTES, prepare_log_file_path,
        rotate_log_file_if_needed,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("sena-cli-{label}-{suffix}"))
    }

    #[test]
    fn prepare_log_file_path_creates_parent_directory() {
        let temp_dir = unique_temp_dir("prepare");
        let log_path = temp_dir.join("logs").join(CLI_LOG_FILE_NAME);

        prepare_log_file_path(log_path.clone()).expect("log path should be prepared");

        assert!(log_path.parent().expect("parent dir").exists());
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn rotate_log_file_moves_large_file_into_archive() {
        let temp_dir = unique_temp_dir("rotate");
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let log_path = temp_dir.join(CLI_LOG_FILE_NAME);
        let archive_path = temp_dir.join(CLI_LOG_ARCHIVE_NAME);
        let oversized = vec![b'x'; (CLI_LOG_MAX_BYTES + 1) as usize];

        fs::write(&log_path, oversized).expect("log file should be written");
        rotate_log_file_if_needed(&log_path).expect("rotation should succeed");

        assert!(!log_path.exists());
        assert!(archive_path.exists());
        let _ = fs::remove_dir_all(temp_dir);
    }
}