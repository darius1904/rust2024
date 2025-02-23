use serde::{Deserialize, Serialize};

pub type Term = String;
pub type DocumentId = u32;
pub type DocumentName = String;

#[derive(Debug, Serialize, Deserialize)]
pub struct FileData {
    /// name of the zip archive
    pub name: DocumentName,
    /// list of files in the zip archive
    pub files: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SearchData {
    pub terms: Vec<Term>,
    pub max_length: Option<usize>,
    pub min_score: Option<f64>,
}

#[derive(Serialize, Deserialize)]
pub struct SearchMatch {
    pub md5: DocumentName,
    pub score: f64,
    pub matched_terms: Vec<Term>,
}

#[derive(Serialize, Deserialize)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
}
