use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use labs2024::{FileData, SearchData, SearchResult};

pub fn main() -> eyre::Result<()> {
    let file = File::open("zip_data.json")?;
    let q = "05bf0e5d5da8cd932cb0460f28defd3f";
    let reader = BufReader::new(file);

    let mut query_terms = vec![];
    for line in reader.lines() {
        let line = line?;
        if !line.contains(q) {
            continue;
        }

        let data: FileData = serde_json::from_str(&line)?;
        for file in data.files {
            for term in file.split("/") {
                query_terms.push(term.to_string());
            }
        }
    }
    query_terms.sort();
    query_terms.dedup();

    if query_terms.is_empty() {
        eprintln!("no terms found for query {q}");
        return Ok(());
    }

    println!("query has {} terms", query_terms.len());
    let req = SearchData {
        terms: query_terms,
        max_length: Some(10),
        min_score: Some(0.5),
    };

    let resp: SearchResult = reqwest::blocking::Client::new()
        .post("http://localhost:8000/search")
        .json(&req)
        .send()?
        .json()?;

    for m in &resp.matches {
        println!("{}: {}", m.md5, m.score);
        for t in &m.matched_terms {
            print!("  {}", t);
        }
        println!("");
    }
    println!("got {} matches", resp.matches.len());
    Ok(())
}
