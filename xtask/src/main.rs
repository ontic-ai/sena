use clap::{Parser, Subcommand};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Build automation for Sena", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Dump workspace diagnostic information
    Dump {
        /// Scope dump to a specific crate (e.g., "bus", "runtime")
        #[arg(long = "crate")]
        crate_name: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Dump { crate_name } => {
            if let Err(e) = dump(crate_name.as_deref()) {
                eprintln!("Error: {}", e);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
    }
}

fn dump(crate_name: Option<&str>) -> Result<(), String> {
    // Workspace root is parent of xtask
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("Cannot determine workspace root")?;
    let crates_dir = workspace_root.join("crates");

    let target_dir = if let Some(name) = crate_name {
        let crate_path = crates_dir.join(name);
        if !crate_path.exists() {
            return Err(format!("Crate '{}' not found in crates/ directory", name));
        }
        crate_path
    } else {
        crates_dir.clone()
    };

    // Collect all .rs files, sorted
    let mut rs_files = BTreeSet::new();
    collect_rs_files(&target_dir, &crates_dir, &mut rs_files)?;

    // Print each file
    for relative_path in rs_files {
        let full_path = crates_dir.join(&relative_path);

        // Convert path to forward slashes for consistency
        let display_path = relative_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        println!("// === {} ===", display_path);

        let content = fs::read_to_string(&full_path)
            .map_err(|e| format!("Failed to read {}: {}", display_path, e))?;

        print!("{}", content);

        // Ensure blank line separator (handle files with/without trailing newline)
        if !content.ends_with('\n') {
            println!();
        }
        println!();
    }

    Ok(())
}

fn collect_rs_files(dir: &Path, base: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let path = entry.path();

        // Skip target directories
        if path.is_dir() && path.file_name() == Some(std::ffi::OsStr::new("target")) {
            continue;
        }

        if path.is_dir() {
            collect_rs_files(&path, base, files)?;
        } else if path.extension() == Some(std::ffi::OsStr::new("rs")) {
            let relative = path
                .strip_prefix(base)
                .map_err(|e| format!("Failed to compute relative path: {}", e))?
                .to_path_buf();
            files.insert(relative);
        }
    }

    Ok(())
}
