#[macro_use]
extern crate rocket;

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Instant,
};

use clap::Parser;

use labs2024::{DocumentId, DocumentName, FileData, SearchMatch, SearchResult, Term};
use rocket::{
    fs::{FileServer, TempFile},
    serde::json::Json,
    State,
};

use rayon::iter::ParallelBridge;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[derive(Default, Serialize, Deserialize)]
struct IndexedData {
    terms_to_docs: HashMap<Term, Vec<DocumentId>>,
    docs_by_id: HashMap<DocumentId, DocumentName>,
    doc_lengths: HashMap<DocumentId, usize>,
    idf: HashMap<Term, f64>,
    num_docs: usize,
}

fn compute_idf(terms_to_docs: &HashMap<Term, Vec<DocumentId>>) -> HashMap<Term, f64> {
    let n = terms_to_docs.len() as f64;
    let mut terms_idf = HashMap::with_capacity(terms_to_docs.len());
    for (term, docs) in terms_to_docs {
        let nq = docs.len() as f64;
        let idf = ((n - nq + 0.5) / (nq + 0.5)).ln();
        terms_idf.insert(term.clone(), idf);
    }

    terms_idf
}

fn load_data(data_filename: impl AsRef<Path>, limit: Option<usize>) -> eyre::Result<IndexedData> {
    let data_filename = data_filename.as_ref();
    let file = File::open(data_filename)?;
    let reader = BufReader::new(file);

    let mut index = reader
        .lines()
        .take(limit.unwrap_or(usize::MAX))
        .enumerate()
        .par_bridge()
        .try_fold(
            || IndexedData::default(),
            |mut acc: IndexedData, (idx, line)| -> eyre::Result<_> {
                let line = line?;
                let terms_map = &mut acc.terms_to_docs;
                let fd: FileData = serde_json::from_str(&line)?;
                let idx: DocumentId = idx.try_into()?;
                let mut num_terms = 0;
                for file in fd.files {
                    for term in file.split("/") {
                        if let Some(set) = terms_map.get_mut(term) {
                            if set.last().copied() != Some(idx) {
                                set.push(idx);
                                num_terms += 1;
                            }
                        } else {
                            terms_map.insert(term.to_string(), vec![idx]);
                            num_terms += 1;
                        }
                    }
                }

                acc.num_docs += 1;
                acc.docs_by_id.insert(idx, fd.name);
                acc.doc_lengths.insert(idx, num_terms);
                Ok(acc)
            },
        )
        .try_reduce(
            || IndexedData::default(),
            |mut acc, chunk| -> eyre::Result<_> {
                for (term, docs) in chunk.terms_to_docs {
                    if let Some(set) = acc.terms_to_docs.get_mut(&term) {
                        set.extend(docs);
                    } else {
                        acc.terms_to_docs.insert(term, docs);
                    }
                }
                acc.docs_by_id.extend(chunk.docs_by_id);
                acc.doc_lengths.extend(chunk.doc_lengths);
                acc.num_docs += chunk.num_docs;
                Ok(acc)
            },
        )?;

    index.idf = compute_idf(&index.terms_to_docs);
    Ok(index)
}

fn run_search(
    data: &IndexedData,
    terms: Vec<Term>,
    min_score: f64,
    max_length: usize,
) -> Vec<SearchMatch> {
    let mut counter: HashMap<DocumentId, Vec<&str>> = HashMap::new();
    for term in &terms {
        if let Some(docs) = data.terms_to_docs.get(term) {
            for doc in docs {
                let x = counter.entry(*doc).or_insert(Vec::new());
                x.push(term.as_str());
            }
        }
    }

    let mut matches = Vec::new();
    for (doc, matched) in counter {
        let doc_name = data
            .docs_by_id
            .get(&doc)
            .cloned()
            .expect("index is broken??");
        let n2 = data.doc_lengths[&doc];

        let union_card = terms.len() + n2 - matched.len();
        let score = matched.len() as f64 / union_card as f64;
        if score < min_score {
            continue;
        }
        matches.push(SearchMatch {
            md5: doc_name,
            score,
            matched_terms: matched.into_iter().map(str::to_string).collect(),
        });
    }
    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    matches.truncate(max_length);
    matches
}

#[derive(FromForm)]
struct Upload<'r> {
    file: TempFile<'r>,
}

#[post("/search_by_file", data = "<upload>")]
fn search_by_file(upload: rocket::form::Form<Upload<'_>>) {
    let file = File::open(upload.file.path().unwrap()).unwrap();
    let reader = BufReader::new(file);
    let mut zip = zip::ZipArchive::new(reader).unwrap();

    let mut filenames = Vec::new();
    for i in 0..zip.len() {
        let file = zip.by_index(i).unwrap();
        filenames.push(file.name().to_string());
    }

    let mut filename_parts = Vec::new();
    for filename in &filenames {
        filename_parts.extend(filename.split("/"));
    }
}

#[post("/search", data = "<req>")]
fn search(
    req: Json<labs2024::SearchData>,
    server_state: &State<Arc<RwLock<ServerState>>>,
) -> Result<Json<SearchResult>, String> {
    let terms = req.terms.clone();
    let min_score = req.min_score.unwrap_or(0.0);
    let server_state = server_state
        .read()
        .map_err(|err| format!("Error: {err:#}"))?;
    let matches = run_search(
        &server_state.index,
        terms,
        min_score,
        req.max_length.unwrap_or(usize::MAX),
    );
    Ok(Json(SearchResult { matches }))
}

#[derive(Default)]
struct ServerState {
    index: IndexedData,
}

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long)]
    build_from: Option<PathBuf>,

    #[arg(long)]
    load_from: Option<PathBuf>,

    #[arg(long)]
    save_to: Option<PathBuf>,

    #[arg(long)]
    limit: Option<usize>,
}

#[rocket::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    let start = Instant::now();
    let data = if let Some(input) = &args.build_from {
        load_data(input, args.limit)?
    } else if let Some(saved) = &args.load_from {
        let file = File::open(saved)?;
        let reader = BufReader::new(file);
        rmp_serde::from_read(reader)?
    } else {
        eprintln!("either input or saved data must be provided");
        std::process::exit(1);
    };

    let pair_count = data
        .terms_to_docs
        .values()
        .map(|docs| docs.len())
        .sum::<usize>();

    println!(
        "loaded data for {} docs, {} terms, {} term-docid pairs, in {:.2}s",
        data.num_docs,
        data.terms_to_docs.len(),
        pair_count,
        start.elapsed().as_secs_f64(),
    );

    if let Some(save_to) = &args.save_to {
        let start = Instant::now();
        let file = File::create(save_to)?;
        let mut file = BufWriter::new(file);
        rmp_serde::encode::write_named(&mut file, &data)?;
        println!("saved data in {:.2}s", start.elapsed().as_secs_f64());
    }

    let server_state = Arc::new(RwLock::new(ServerState { index: data }));
    rocket::build()
        .manage(server_state)
        .mount("/", routes![search, search_by_file])
        .mount("/dashboard", FileServer::from("static"))
        .ignite()
        .await?
        .launch()
        .await?;

    Ok(())
}
