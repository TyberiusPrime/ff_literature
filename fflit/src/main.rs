mod assemble;
mod bibtex;
mod crossref;
mod datacite;
mod diff;
mod discover;
mod error;
mod metadata;
mod pdf;
mod scan;
mod search;
mod text;

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
    Reindex {
        /// only refresh the keywords from literature.bibtex, leave the text alone
        #[arg(long)]
        tags_only: bool,
    },
    /// Report the entries of one bibtex that are missing from another
    Diff {
        /// bibtex file (or fflit repository) to take entries from
        a: PathBuf,
        /// bibtex file (or fflit repository) to look them up in
        b: PathBuf,
        /// write the missing entries to this bibtex file
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// put the uncertain entries into --output as well
        #[arg(long)]
        include_uncertain: bool,
    },
    /// Collect the references cited by typst documents into a standalone bibtex (+ pdfs)
    Assemble {
        /// fflit repository to pull entries from (the directory holding literature.bibtex)
        repository: PathBuf,
        /// bibtex file to write
        output: PathBuf,
        /// typst files to scan for @references
        #[arg(required = true)]
        typst_files: Vec<PathBuf>,
        /// also copy the cited pdfs into this directory
        #[arg(long, value_name = "DIR")]
        pdf_dir: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan => scan::scan()?,
        Command::Add { path, doi } => scan::add_with_doi(&path, &doi)?,
        Command::Search { query, context } => search::search(&query, context)?,
        Command::Reindex { tags_only } => match tags_only {
            true => search::reindex_tags()?,
            false => search::reindex()?,
        },
        Command::Diff {
            a,
            b,
            output,
            include_uncertain,
        } => diff::diff(&a, &b, output.as_deref(), include_uncertain)?,
        Command::Assemble {
            repository,
            output,
            typst_files,
            pdf_dir,
        } => {
            let ok = assemble::assemble(&repository, &output, pdf_dir.as_deref(), &typst_files)?;
            if !ok {
                // unresolved references: output was still written
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
