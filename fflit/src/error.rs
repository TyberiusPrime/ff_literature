use thiserror::Error;

#[derive(Error, Debug)]
pub enum FflitError {
    #[error("no DOI found in PDF")]
    NoDoi,
    #[error("metadata fetch failed: {0}")]
    MetadataFetch(String),
    #[error("PDF read error: {0}")]
    PdfRead(String),
}
