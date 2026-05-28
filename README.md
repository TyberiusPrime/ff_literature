# fflit

Personal literature 'manager'. 

* Scans an `incoming/` drop folder
* extracts DOIs from PDFs
* fetches metadata from CrossRef 
* renames/moves files to `pdfs/<BibKey>.pdf`
* maintains a sorted `literature.bibtex`. 

Full-text search via tantivy.

No document without pdf.

Vibe coded house plant software.

It's mostly glue between existing things anyway.  

## Requirements

- `pdftotext` (poppler) — in PATH
- Network access to `api.crossref.org`

With the Nix flake: `nix develop` puts everything in scope.

## Usage

Library = directory.

Put your pdfs into directory/incoming.

Run 'fflit scan'.

directory/literature.bibtex get's updated.

PDFs are placed in pdfs (or in failed_pdfs if no doi extraction was possible).

```
fflit scan
```
Process every PDF in `./incoming/`. For each file:
1. SHA-256 checked against `literature.bibtex` — duplicate goes to `duplicates/`
2. DOI extracted from PDF metadata, then from first 3 pages of text
3. No DOI → `failed_pdfs/`
4. Metadata fetched from CrossRef; fetch failure → `failed_pdfs/`
5. File moved to `pdfs/<Author><Year><Word>.pdf` and entry appended to `literature.bibtex`

```
fflit add path/to/paper.pdf --doi 10.xxxx/xxxxx
```
Manually file a PDF when DOI extraction fails.

```
fflit search "query terms"
fflit search "query terms" --context
```
Full-text search. Prints matching `./pdfs/<key>.pdf` with title and first author.
`--context` adds up to 3 matching passages per result with query terms highlighted.

```
fflit reindex
```
Rebuild the tantivy full-text index from scratch (needed after first use or schema changes).

## Directory layout

```
.
├── incoming/          # drop PDFs here
├── pdfs/              # filed papers: Author2024Word.pdf
├── duplicates/        # sha256 or DOI already known
├── failed_pdfs/       # no DOI found, or CrossRef returned nothing
├── literature.bibtex  # sorted BibTeX database
└── search_index/      # tantivy index (do not edit)
```

## BibTeX key format

`AuthorYYYYWord` — first author family name, 4-digit year, first significant title word.
Conflicts gain a suffix: `Smith2023Attentiona`, `Smith2023Attentionb`, …

Each entry includes a `sha256` field for deduplication across renames.

## Building

```sh
nix develop        # enter dev shell
cargo build        # debug
nix build          # release via naersk
```

## License

MIT — see [LICENSE](LICENSE).
