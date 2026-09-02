# fflit

Personal literature 'manager'. 

* Scans an `incoming/` drop folder
* works out what each PDF is — DOI, arXiv id, ISBN, or by asking about its title page
* fetches metadata from CrossRef, DataCite, doi.org or OpenLibrary 
* renames/moves files to `pdfs/<BibKey>.pdf`
* maintains a sorted `literature.bibtex`. 

Full-text search via tantivy.

No document without pdf.

Vibe coded house plant software.

It's mostly glue between existing things anyway.  

## Requirements

- `pdftotext` (poppler) — in PATH
- Network access to `api.crossref.org`, `api.datacite.org`, `doi.org` and
  `openlibrary.org`

With the Nix flake: `nix develop` puts everything in scope.

## Usage

Library = directory.

Put your pdfs into directory/incoming.

Run 'fflit scan'.

directory/literature.bibtex get's updated.

PDFs are placed in pdfs (or in failed_pdfs if no doi extraction was possible).

```
fflit scan
fflit scan --tag immunology --tag mouse
fflit scan --tag "immunology, mouse"
```
Process every PDF in `./incoming/`. For each file:
1. SHA-256 checked against `literature.bibtex` — duplicate goes to `duplicates/`
2. Identified (see below); failure → `failed_pdfs/`
3. Metadata fetched (see below); fetch failure → `failed_pdfs/`
4. File moved to `pdfs/<Author><Year><Word>.pdf` and entry appended to `literature.bibtex`

### Where metadata comes from

A DOI is resolved by asking, in order:

1. **CrossRef** — the published literature, and the richest records.
2. **DataCite** — arXiv, Zenodo, datasets, and a good deal else besides.
3. **doi.org content negotiation** — every registration agency answers this with
   CSL-JSON, so the agencies fflit does not speak to directly are covered by one
   fallback rather than one API each. This is what resolves ISTIC, mEDRA, JaLC,
   KISTI and Airiti DOIs, which return 404 from both CrossRef and DataCite.

If all three come up empty, the DOI is looked up in the registration agency
index, so the error distinguishes a DOI nobody can describe from one that was
never registered:

```
  metadata fetch failed (10.9999/nope is not a registered DOI)
  metadata fetch failed (10.xxxx/yyy is registered with mEDRA, but none of
                         crossref, datacite, doi.org could describe it)
```

Books are resolved by ISBN through CrossRef, then OpenLibrary.

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
6. **ISBN in the front matter** (pages 1-8 — a book prints it on the copyright
   page, not the cover). Only labelled ISBNs with a valid check digit count, so
   the LCCN and the phone number on the same page are not mistaken for one.
   Resolved through CrossRef, which has the academic presses and yields a DOI as
   well; failing that through OpenLibrary. Several ISBNs on a page (paperback,
   hardback, ebook) are tried in the order printed.
7. **DOI anywhere else in the document.** This one is *not* believed on its
   own — a DOI in the body is usually a reference to somebody else's paper —
   so the metadata it buys has to describe the title page before it is used.
8. **The title page itself.** The title is read off the page (skipping running
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
fflit add path/to/book.pdf --isbn 978-0-262-03384-8 --tag methods
```
Manually file a PDF when identification fails. ISBN-10 is accepted and converted.

### Tags

`--tag` puts a `keywords` field on everything a run files — drop a batch of
papers on one topic into `incoming/`, tag the batch in one go. Tags are searched
alongside title, authors and full text, and shown after each hit, with no
reindex needed:

```
$ fflit scan --tag "transformers, to-read"
  added → ./pdfs/Vaswani2017Attention.pdf [transformers, to-read]

$ fflit search to-read
./pdfs/Vaswani2017Attention.pdf  Attention Is All You Need — Vaswani  [transformers, to-read]
```

fflit only ever writes tags when it files a pdf. Changing the tags of something
already in the library is an edit to `literature.bibtex` followed by
`fflit reindex --tags-only`. A pdf in a tagged batch that turns out to be a
duplicate is filed to `duplicates/` as usual and the entry you already have is
left exactly as it is.

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
fflit fetch missing.bibtex
fflit fetch missing.bibtex --dry-run
fflit fetch missing.bibtex --limit 20 --into ./incoming
fflit fetch missing.bibtex --publisher --worklist todo.tsv
```
Download the open access copies of everything in a bibtex.

**What the library already holds is subtracted first**, so pointing this at your
whole bibliography is safe and idempotent — it will not re-download what you
have. Entries are recognised by DOI, by ISBN, and by title, the last because the
same paper can sit in two files under two different DOIs (a preprint filed from
its arXiv id, wanted under the journal DOI). `--repository DIR` says which
library to subtract; the default is the current directory.

```
$ fflit fetch everything.bibtex --dry-run
oa      new_one_2020  (repository landing page)

1 available, 0 closed access, 0 failed
3 skipped, already in the library (1 by doi, 2 by title)
```

It also pairs with `diff`, which is worth doing for the report rather than the
subtraction:

```sh
fflit diff theirs.bibtex . --output missing.bibtex   # what the library lacks
fflit fetch missing.bibtex                           # get what is free
fflit scan --tag from-unpaywall                      # file it
```

Unpaywall is asked where a legally free copy lives, and every location it names
is tried in turn — published version before accepted manuscript before preprint,
repositories before publishers, since a repository will hand a file to a script
and a publisher often will not. Locations with only a landing page are tried
last, because sometimes that page *is* the pdf.

Every download is checked for actually being one: a captcha, a cookie wall or an
apology gets rejected rather than filed, so `failed` means no location served a
real file, not that the paper is unavailable.

DOI fields are normalised before any registry sees them — `https://doi.org/…`
and `doi:` prefixes stripped, whitespace trimmed, case folded — and entries
whose `doi` field holds something that is not a DOI (`in press`, a url, an empty
brace pair) are counted and skipped rather than sent. This matters more than it
sounds: the PubMed Central converter rejects an entire request of 200 over one
id it dislikes, so a single malformed field used to cost every lookup in its
batch. Batches that are rejected anyway are halved and retried rather than lost.

**PubMed Central** is checked as well, in one batched lookup, since it holds
free copies Unpaywall does not always list — NIH funded author manuscripts above
all. PMC serves an interstitial rather than a file to anything that is not a
browser, so those downloads usually fail; fflit reports how many of your papers
are free to read there and puts the links in the worklist rather than pretending
to be a browser to get around it.

**`--publisher`** is for when you are on a network your library subscribes from.
It resolves the DOI, reads the `citation_pdf_url` that publishers advertise for
indexing, and asks for that file — no circumvention, and off the subscribing
network it just gets a paywall page and gives up. Expect partial success:

| publisher | landing page, plain HTTP client |
| --- | --- |
| Nature, PLOS | serves HTML, tag present ✓ |
| Elsevier | JS redirect, no tag |
| ACM, OUP | 403 Cloudflare challenge |

Cloudflare challenges the client rather than the IP, so a subscribing network
does not help there. Two seconds between publisher requests, deliberately:
publishers watch for exactly this traffic and block whole campuses over it.

**What could not be fetched is printed as links to chase up yourself** — every
place fflit tried, not just one, since those are the pages you would open. Up to
four per paper, in the order they were tried, grouped by what stood in the way:

```
to chase up yourself:

  bot challenge, opens fine in a browser — 2 paper(s)
    Oxford2021Nar  Something at Oxford University Press
      https://academic.oup.com/nar/article-pdf/49/D1/D480/35364103/gkaa1100.pdf  (publisher published version (refused a download))
      https://archive-ouverte.unige.ch/unige:159643  (repository landing page (refused a download))
      https://repository.publisso.de/resource/frl:6425514  (repository landing page (refused a download))

  pdf refused, may work from the subscribing network — 1 paper(s)
    LeCun2015Deep  Deep learning
      https://hal.science/hal-04206682  (repository landing page (refused a download))
      https://www.nature.com/articles/nature14539  (publisher page)
```

The grouping is the useful part: a Cloudflare challenge or an Elsevier
javascript redirect is nothing to a browser, so those are worth clicking, while
`closed access` means nobody has a copy to give. Session noise publishers hang
off the url (`?error=cookies_not_supported&code=…`) is stripped, since it is
stale by the time anyone clicks. A repository landing page that refused a script
will usually hand a person the pdf.

**`--worklist FILE`** writes the same thing as a header row plus
`key/title/url/found/reason`, one row per link:

```sh
fflit fetch missing.bibtex --publisher --worklist chase.tsv
cut -f3 chase.tsv | tail -n +2 | xargs -n1 -P4 firefox     # open the lot
grep 'browser' chase.tsv | cut -f3                          # only what a browser gets through
awk -F'\t' '$5 ~ /pubmed/' chase.tsv | cut -f3             # only pubmed central
```

Files are named after their citation key, so an interrupted run resumes where it
stopped — already downloaded entries are skipped. `--dry-run` reports what is
available without downloading, `--limit` stops after N lookups. Nothing here
touches `literature.bibtex`; the pdfs land in `incoming/` and are `fflit scan`'s
problem from there.

```
fflit repair
fflit repair --dry-run
```
Fix a `literature.bibtex` that no longer parses. The usual cause is unbalanced
braces in a field value: values are written inside `{…}`, so one brace that
never closes — or closes nothing — swallows the rest of that entry and every
entry after it. Abstracts are the usual source; markup arrives from the registry
already mangled, and a single stray `}` in `O(n log} | Σ |)` is enough.

```
$ fflit scan
Error: parsing ./literature.bibtex: unbalanced braces in one entry
  line 7935: @{Vlimki2007Compressed}, in the abstract field
run `fflit repair ./literature.bibtex` to fix them

$ fflit repair
one entry with unbalanced braces:
  line 7935: Vlimki2007Compressed, in the abstract field
1 field(s) fixed in ./literature.bibtex (previous version kept as ./literature.bibtex.bak)
```

Unmatched braces are dropped rather than escaped — bibtex parsers count raw
braces, so `\{` would leave the file just as unbalanced, and a delimiter with
nothing to delimit has no meaning to lose. Balanced braces are left alone, since
`{DNA}` in a title protects capitalisation on purpose. The repair works on lines
rather than by parsing, since a file that needs repairing is by definition one
that will not parse, and it refuses to write unless the result balances. The
previous version is always kept as `.bibtex.bak`.

Entries written from now on are brace balanced on the way out, so this should
not recur.

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

## Books

Academic publishers register DOIs for books and chapters — Springer, Elsevier,
OUP, CUP, the university presses — and those come in through the DOI paths like
anything else. Technical and trade publishers do not, and neither does most of
what was printed before DOIs existed; those are identified by ISBN and get an
`isbn` field where an article would have a `doi`:

```bibtex
@book{Buffalo2015Bioinformatics,
  author = {Buffalo, Vince},
  title = {Bioinformatics Data Skills: Reproducible and Robust Research with Open Source Tools},
  year = {2015},
  isbn = {9781449367374},
  publisher = {O'Reilly Media, Incorporated},
  sha256 = {62f83c632d545e3bf9fcea93614de372dd925f1a7d7c740b2a2b0e0aadc7c1d5},
}
```

ISBNs copied out of a book are accepted however they are typeset — non-breaking
hyphens, en dashes and figure dashes instead of `-`, and digits that are not
ASCII at all (fullwidth, mathematical, Arabic-Indic and the other decimal
blocks). All forms fold to the same 13 digits, so the same book pasted from two
sources is still one entry. DOIs read out of a PDF get the same treatment.

A book CrossRef does know gets both fields. ISBNs are stored as plain 13 digits:
correct hyphenation depends on the registration group ranges, and a plausible
looking but wrong grouping is worse than none. ISBN-10s are converted on the way
in, so the same book found under either form is recognised as a duplicate.

## BibTeX key format

`AuthorYYYYWord` — first author family name, 4-digit year, first significant title word.
Conflicts gain a suffix: `Smith2023Attentiona`, `Smith2023Attentionb`, …

Each entry includes a `sha256` field for deduplication across renames; entries
are also deduplicated by DOI and by ISBN. A
hand-written `keywords` field is searchable after `fflit reindex --tags-only`.

## Building

```sh
nix develop        # enter dev shell
cargo build        # debug
nix build          # release via naersk
```

## License

MIT — see [LICENSE](LICENSE).
