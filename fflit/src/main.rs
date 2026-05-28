mod bibtex;
mod crossref;
mod error;
mod pdf;
mod scan;
mod search;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "fflit", about = "Personal literature manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan ./incoming/ for new PDFs, extract DOIs, fetch metadata, file them
    Scan,
    /// Manually add a PDF with a known DOI
    Add {
        path: PathBuf,
        #[arg(long)]
        doi: String,
    },
    /// Full-text search; prints matching ./pdfs/<key>.pdf paths with title and first author
    Search {
        query: String,
        /// Show up to 3 matching text passages per result
        #[arg(long)]
        context: bool,
    },
    /// Rebuild the full-text search index from ./pdfs/
    Reindex,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan => scan::scan()?,
        Command::Add { path, doi } => scan::add_with_doi(&path, &doi)?,
        Command::Search { query, context } => search::search(&query, context)?,
        Command::Reindex => search::reindex()?,
    }
    Ok(())
}
