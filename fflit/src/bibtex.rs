use anyhow::Context;
use nom_bibtex::Bibtex;
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone)]
pub struct BibEntry {
    pub entry_type: String,
    pub key: String,
    pub fields: Vec<(String, String)>,
}

impl BibEntry {
    /// Field lookup, case insensitive in the field name as bibtex is.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The `keywords` field, split on commas.
    pub fn keywords(&self) -> Vec<String> {
        split_keywords(self.field("keywords").unwrap_or_default())
    }
}

/// bibtex has no formal syntax here; comma separated is what everyone uses.
pub fn split_keywords(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

pub struct BibDatabase {
    pub entries: Vec<BibEntry>,
    doi_set: HashSet<String>,
    isbn_set: HashSet<String>,
    sha256_set: HashSet<String>,
}

impl BibDatabase {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let bibtex = Bibtex::parse(&content)
            .map_err(|e| anyhow::anyhow!("parsing {}: {:?}", path.display(), e))?;

        let mut entries = Vec::new();
        let mut doi_set = HashSet::new();
        let mut isbn_set = HashSet::new();
        let mut sha256_set = HashSet::new();

        for bib in bibtex.bibliographies() {
            let mut fields: Vec<(String, String)> = bib
                .tags()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            // stable order for re-serialisation
            fields.sort_by(|a, b| field_order_key(&a.0).cmp(&field_order_key(&b.0)));

            for (k, v) in &fields {
                match k.to_lowercase().as_str() {
                    "doi" => {
                        doi_set.insert(normalize_doi(v));
                    }
                    "isbn" => {
                        // stored however it was printed, compared as digits
                        isbn_set.extend(crate::isbn::normalize(v));
                    }
                    "sha256" => {
                        sha256_set.insert(v.clone());
                    }
                    _ => {}
                }
            }

            entries.push(BibEntry {
                entry_type: bib.entry_type().to_string(),
                key: bib.citation_key().to_string(),
                fields,
            });
        }

        Ok(Self {
            entries,
            doi_set,
            isbn_set,
            sha256_set,
        })
    }

    pub fn empty() -> Self {
        Self {
            entries: vec![],
            doi_set: HashSet::new(),
            isbn_set: HashSet::new(),
            sha256_set: HashSet::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&BibEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// For "did you mean" hints on unknown citation keys.
    pub fn find_ignore_case(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case(key))
            .map(|e| e.key.as_str())
    }

    pub fn contains_doi(&self, doi: &str) -> bool {
        !doi.is_empty() && self.doi_set.contains(&normalize_doi(doi))
    }

    pub fn contains_isbn(&self, isbn: &str) -> bool {
        crate::isbn::normalize(isbn).is_some_and(|i| self.isbn_set.contains(&i))
    }

    pub fn contains_sha256(&self, hash: &str) -> bool {
        self.sha256_set.contains(hash)
    }

    pub fn key_exists(&self, key: &str) -> bool {
        self.entries.iter().any(|e| e.key == key)
    }

    pub fn generate_key(&self, author_family: &str, year: u32, title: &str) -> String {
        let author = ascii_name(author_family);
        let word = first_significant_word(title);
        let base = format!("{}{}{}", author, year, word);

        if !self.key_exists(&base) {
            return base;
        }
        for c in b'a'..=b'z' {
            let candidate = format!("{}{}", base, c as char);
            if !self.key_exists(&candidate) {
                return candidate;
            }
        }
        panic!("exhausted 26 conflict suffixes for key {base} — something is very wrong");
    }

    pub fn add(&mut self, entry: BibEntry) {
        for (k, v) in &entry.fields {
            match k.to_lowercase().as_str() {
                "doi" => {
                    self.doi_set.insert(normalize_doi(v));
                }
                "isbn" => {
                    self.isbn_set.extend(crate::isbn::normalize(v));
                }
                "sha256" => {
                    self.sha256_set.insert(v.clone());
                }
                _ => {}
            }
        }
        self.entries.push(entry);
    }

    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| a.key.cmp(&b.key));

        let mut out = String::new();
        for entry in &sorted {
            out.push_str(&format!("@{}{{{},\n", entry.entry_type, entry.key));
            for (k, v) in &entry.fields {
                out.push_str(&format!("  {} = {{{}}},\n", k, v));
            }
            out.push_str("}\n\n");
        }

        std::fs::write(path, out)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

fn normalize_doi(s: &str) -> String {
    s.trim_start_matches("https://doi.org/")
        .trim_start_matches("http://doi.org/")
        .trim_start_matches("doi:")
        .to_lowercase()
}

fn ascii_name(s: &str) -> String {
    let clean: String = s.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let mut chars = clean.chars();
    match chars.next() {
        None => String::from("Unknown"),
        Some(c) => c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
    }
}

pub const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "for", "with", "to", "at", "by", "from", "and", "or",
    "is", "are", "was", "were", "be", "been", "that", "this", "these", "those", "it", "its",
    "via", "into", "over", "under", "toward", "using",
];

fn first_significant_word(title: &str) -> String {
    for word in title.split_whitespace() {
        let clean: String = word.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        if clean.is_empty() {
            continue;
        }
        if STOP_WORDS.contains(&clean.to_lowercase().as_str()) {
            continue;
        }
        let mut chars = clean.chars();
        return match chars.next() {
            None => String::from("Unknown"),
            Some(c) => c.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
        };
    }
    String::from("Unknown")
}

// lower number = written earlier in the entry
fn field_order_key(f: &str) -> u8 {
    match f.to_lowercase().as_str() {
        "author" => 0,
        "title" => 1,
        "year" => 2,
        "doi" => 3,
        "isbn" => 4,
        "journal" => 5,
        "booktitle" => 5,
        "volume" => 6,
        "number" => 7,
        "pages" => 8,
        "publisher" => 9,
        "abstract" => 10,
        "keywords" => 11,
        "sha256" => 12,
        _ => 50,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isbns_match_however_they_are_written() {
        let mut db = BibDatabase::empty();
        db.add(BibEntry {
            entry_type: "book".into(),
            key: "Buffalo2015".into(),
            fields: vec![("isbn".into(), "978-1-4493-6737-4".into())],
        });
        assert!(db.contains_isbn("9781449367374"));
        assert!(db.contains_isbn("978 1 4493 6737 4"));
        assert!(!db.contains_isbn("9780596520687"));
        // not an isbn at all, so not a match
        assert!(!db.contains_isbn("garbage"));
    }

    #[test]
    fn an_empty_doi_matches_nothing() {
        let mut db = BibDatabase::empty();
        db.add(BibEntry {
            entry_type: "book".into(),
            key: "Buffalo2015".into(),
            fields: vec![("isbn".into(), "9781449367374".into())],
        });
        assert!(!db.contains_doi(""));
    }

    #[test]
    fn keywords_are_split_on_commas() {
        let entry = BibEntry {
            entry_type: "article".into(),
            key: "Smith2020Deep".into(),
            fields: vec![("Keywords".into(), " mouse ,, immunology ".into())],
        };
        assert_eq!(entry.keywords(), vec!["mouse", "immunology"]);

        let untagged = BibEntry {
            entry_type: "article".into(),
            key: "Smith2020Deep".into(),
            fields: vec![("title".into(), "T".into())],
        };
        assert!(untagged.keywords().is_empty());
    }
}
