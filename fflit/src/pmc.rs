//! PubMed Central, for the free full text that Unpaywall does not always know
//! about — NIH deposited author manuscripts in particular.
//!
//! PMC serves an interstitial rather than a file to anything that is not a
//! browser, and asks bulk users to go through its OA service instead. fflit
//! therefore offers the link and lets the download fail honestly, rather than
//! pretending to be a browser to get around it.

use anyhow::Context;
use std::collections::HashMap;

const USER_AGENT: &str = "fflit/0.1 (mailto:john@coonabibba.de; https://github.com/fflit)";
/// NCBI's converter takes this many ids at a time.
const BATCH: usize = 200;

/// DOI → PMCID for those that have one. Batched, because NCBI asks for no more
/// than three requests a second and 1000 lookups one at a time is rude.
///
/// Never fails as a whole: the converter rejects an entire request over one id
/// it dislikes, so a rejected batch is halved and retried rather than lost.
pub fn pmcids(dois: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for chunk in dois.chunks(BATCH) {
        resolve(chunk, &mut out);
    }
    out
}

fn resolve(chunk: &[String], out: &mut HashMap<String, String>) {
    if chunk.is_empty() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(350));
    match lookup(chunk) {
        Ok(found) => out.extend(found),
        Err(_) if chunk.len() > 1 => {
            // one of them is unpalatable; find out which half
            let (a, b) = chunk.split_at(chunk.len() / 2);
            resolve(a, out);
            resolve(b, out);
        }
        // a single id the converter will not take: nothing more to learn
        Err(_) => {}
    }
}

fn lookup(dois: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    let json: serde_json::Value = reqwest::blocking::Client::new()
        .get("https://pmc.ncbi.nlm.nih.gov/tools/idconv/api/v1/articles/")
        .header("User-Agent", USER_AGENT)
        .query(&[
            ("ids", dois.join(",").as_str()),
            ("format", "json"),
            ("tool", "fflit"),
            ("email", "john@coonabibba.de"),
        ])
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .context("HTTP request failed for the PMC id converter")?
        .error_for_status()
        .context("the PMC id converter returned an error")?
        .json()
        .context("failed to parse the PMC id converter response")?;

    Ok(json["records"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .filter_map(|r| {
                    // records without a pmcid carry an errmsg instead
                    Some((r["doi"].as_str()?.to_string(), r["pmcid"].as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Where a human can read it, and where a download may be refused.
pub fn article_url(pmcid: &str) -> String {
    format!("https://pmc.ncbi.nlm.nih.gov/articles/{pmcid}/")
}

pub fn pdf_url(pmcid: &str) -> String {
    format!("https://pmc.ncbi.nlm.nih.gov/articles/{pmcid}/pdf/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_built_from_the_pmcid() {
        assert_eq!(article_url("PMC11731862"), "https://pmc.ncbi.nlm.nih.gov/articles/PMC11731862/");
        assert_eq!(pdf_url("PMC11731862"), "https://pmc.ncbi.nlm.nih.gov/articles/PMC11731862/pdf/");
    }
}
