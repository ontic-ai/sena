use clap::{Parser, Subcommand};

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
    Dump,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Dump => {
            println!("xtask dump: not implemented");
            std::process::exit(0);
        }
    }
}
