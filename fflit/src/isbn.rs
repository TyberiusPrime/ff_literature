//! Reading ISBNs off a copyright page.
//!
//! That page is mostly numbers — printing codes, LCCN, phone numbers — so the
//! check digit does the real work of telling an ISBN from the rest.

use regex::Regex;
use std::sync::OnceLock;

static ISBN_RE: OnceLock<Regex> = OnceLock::new();

fn isbn_regex() -> &'static Regex {
    ISBN_RE.get_or_init(|| {
        // an ISBN is always announced as one; unlabelled 13 digit runs are not
        // worth the false positives
        Regex::new(r"(?i)ISBN(?:-?1[03])?\s*:?\s*((?:97[89][\s-]?)?(?:\d[\s-]?){9}[\dXx])").unwrap()
    })
}

/// Every valid ISBN in the text, normalised to 13 digits, in the order printed.
/// Books list several — paperback, hardback, ebook — and the first is usually
/// the edition in hand.
pub fn find_all(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in isbn_regex().captures_iter(text) {
        let Some(isbn) = normalize(&c[1]) else { continue };
        if !out.contains(&isbn) {
            out.push(isbn);
        }
    }
    out
}

/// Strip separators, check the digit, and return the ISBN-13 form.
pub fn normalize(raw: &str) -> Option<String> {
    let digits: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
        .map(|c| c.to_ascii_uppercase())
        .collect();

    match digits.len() {
        10 if check10(&digits) => Some(to13(&digits)),
        13 if check13(&digits) => Some(digits),
        _ => None,
    }
}

fn check10(isbn: &str) -> bool {
    let sum: u32 = isbn
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let v = if c == 'X' { 10 } else { c.to_digit(10).unwrap_or(0) };
            (10 - i as u32) * v
        })
        .sum();
    sum % 11 == 0
}

fn check13(isbn: &str) -> bool {
    let sum: u32 = isbn
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let w = if i % 2 == 0 { 1 } else { 3 };
            w * c.to_digit(10).unwrap_or(0)
        })
        .sum();
    sum % 10 == 0
}

fn to13(isbn10: &str) -> String {
    let body = format!("978{}", &isbn10[..9]);
    let sum: u32 = body
        .chars()
        .enumerate()
        .map(|(i, c)| (if i % 2 == 0 { 1 } else { 3 }) * c.to_digit(10).unwrap_or(0))
        .sum();
    format!("{}{}", body, (10 - sum % 10) % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphens_and_spaces_do_not_matter() {
        assert_eq!(find_all("ISBN 978-1-4493-6737-4"), vec!["9781449367374"]);
        assert_eq!(find_all("ISBN: 978 1 4493 6737 4"), vec!["9781449367374"]);
        assert_eq!(find_all("isbn 9781449367374"), vec!["9781449367374"]);
    }

    #[test]
    fn a_wrong_check_digit_is_not_an_isbn() {
        // the copyright page number that is one digit off is not the book
        assert!(find_all("ISBN 978-1-4493-6737-5").is_empty());
        assert!(normalize("9781449367375").is_none());
    }

    #[test]
    fn isbn_ten_is_converted() {
        assert_eq!(normalize("0-596-52068-9").as_deref(), Some("9780596520687"));
        // the X check digit
        assert_eq!(normalize("043942089X").as_deref(), Some("9780439420891"));
        assert_eq!(normalize("0262033844").as_deref(), Some("9780262033848"));
    }

    #[test]
    fn a_copyright_page_yields_its_editions_in_order() {
        let page = "Copyright © 2015 Vince Buffalo. All rights reserved.\n\
                    Printed in the United States of America.\n\
                    Published by O'Reilly Media, Inc., 1005 Gravenstein Highway North.\n\
                    Telephone: 800-998-9938 or 707-829-0515\n\
                    Library of Congress Control Number: 2015944619\n\
                    ISBN: 978-1-449-36737-4 (paperback)\n\
                    ISBN 978-0-596-52068-7 (ebook)";
        assert_eq!(find_all(page), vec!["9781449367374", "9780596520687"]);
    }

    #[test]
    fn phone_numbers_and_control_numbers_are_not_isbns() {
        assert!(find_all("Telephone: 800-998-9938").is_empty());
        assert!(find_all("Library of Congress Control Number: 2015944619").is_empty());
        assert!(find_all("Printed 10 9 8 7 6 5 4 3 2 1").is_empty());
    }

    #[test]
    fn the_same_isbn_twice_is_listed_once() {
        assert_eq!(
            find_all("ISBN 9781449367374 ... ISBN: 978-1-4493-6737-4"),
            vec!["9781449367374"]
        );
    }
}
