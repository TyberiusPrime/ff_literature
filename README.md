# fflit

Personal literature 'manager'. 

* Scans an `incoming/` drop folder
* works out what each PDF is — DOI, arXiv id, or by asking about its title page
* fetches metadata from CrossRef or DataCite 
* renames/moves files to `pdfs/<BibKey>.pdf`
* maintains a sorted `literature.bibtex`. 

Full-text search via tantivy.

No document without pdf.

Vibe coded house plant software.

It's mostly glue between existing things anyway.  

## Requirements

- `pdftotext` (poppler) — in PATH
- Network access to `api.crossref.org` and `api.datacite.org`

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
2. Identified (see below); failure → `failed_pdfs/`
3. Metadata fetched from CrossRef, or DataCite for what CrossRef does not
   register (arXiv, Zenodo); fetch failure → `failed_pdfs/`
4. File moved to `pdfs/<Author><Year><Word>.pdf` and entry appended to `literature.bibtex`

### How a PDF is identified

In order, stopping at the first that works:

1. **DOI in the info dictionary** — any key whose name mentions doi, then
   `Subject`/`Keywords`/`Title`.
2. **DOI in the XMP packet** — `prism:doi` or `dc:identifier`. This is where
   most publishers actually put it, PLOS and Elsevier among them.
3. **DOI on page 1.** An announced one (`doi:…`, `https://doi.org/…`) beats a
   bare match, because the announced one is the paper's own rather than the
   journal's boilerplate.
4. **arXiv id on page 1** → `10.48550/arXiv.<id>`, resolved through DataCite.
   Both the old (`hep-th/9901001`) and new (`1706.03762`) schemes.
5. **DOI on pages 2-3.**
6. **DOI anywhere else in the document.** This one is *not* believed on its
   own — a DOI in the body is usually a reference to somebody else's paper —
   so the metadata it buys has to describe the title page before it is used.
7. **The title page itself.** The title is read off the page (skipping running
   heads, download stamps and licence lines), CrossRef is asked what work that
   is, and the answer is only accepted if nearly all of its title is on the
   page. Corroborating the first author's name there lowers the bar; a title of
   one or two words never identifies anything.

Whatever is not identified goes to `failed_pdfs/`. If a search candidate was
close but not convincing, it is printed as a guess, with the `fflit add` line
to run if you agree with it:

```
processing: ./incoming/scan_0042.pdf
  note: 10.1038/nature14539 from body text looks like a citation, not this paper
  no DOI found, and the best title match is not convincing — moving to ./failed_pdfs/
  guess: 71% 10.1234/abc "Seasonal wobbling in zebrafish" — fflit add ./failed_pdfs/scan_0042.pdf --doi 10.1234/abc
```

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
fflit assemble <repository> <output.bibtex> [--pdf-dir DIR] paper.typ chapter.typ ...
```
Scan typst files for `@key` citations and collect the matching entries from
`<repository>/literature.bibtex` into `<output.bibtex>`. With `--pdf-dir`, the
cited `<repository>/pdfs/<key>.pdf` are copied there as well.

Package specs (`@preview/...`), mail addresses, raw blocks and labels the
document defines itself (`<fig:one>`) are not treated as citations.

Unknown keys are reported as warnings and make fflit exit non-zero — the output
files are still written. A cited entry without a pdf is a note only.

```
fflit diff <a.bibtex> <b.bibtex> [--output missing.bibtex] [--include-uncertain]
```
Which entries of `a` are not in `b`? Either side may also be an fflit repository
directory, in which case its `literature.bibtex` is used.

Entries are matched by DOI first, then by normalized title (LaTeX accents,
braces, case and punctuation are ignored). Everything else is scored by title
word overlap, first author and year, and lands in one of two buckets:

* **uncertain** — something over there looks like it, printed with the candidate
  and a score. Truncated titles, subtitles, `Thomas S.` vs `Thomas`, an entry
  whose citation key happens to exist on the other side. Your eyes, not mine.
* **missing** — nothing in `b` resembles it.

`--output` writes the missing entries as a bibtex file; `--include-uncertain`
puts the uncertain ones in there too.

```
fflit reindex
fflit reindex --tags-only
```
Rebuild the tantivy full-text index from scratch (needed after first use or schema changes).

`--tags-only` is the cheap variant for keywords. Tags are not managed by fflit —
write a `keywords = {immunology, mouse}` field into `literature.bibtex` by hand,
then run this to make them searchable. Only entries whose tags actually differ
from the index are rewritten, so it costs one `pdftotext` per edited entry
instead of one per paper, and nothing at all when there is nothing to do.

Keywords are searched alongside title, authors and full text, and shown after
each hit. Order and case in the field are not significant.

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

Each entry includes a `sha256` field for deduplication across renames. A
hand-written `keywords` field is searchable after `fflit reindex --tags-only`.

## Building

```sh
nix develop        # enter dev shell
cargo build        # debug
nix build          # release via naersk
```

## License

MIT — see [LICENSE](LICENSE).
