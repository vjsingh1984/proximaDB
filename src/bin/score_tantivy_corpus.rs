//! Seal a source-level lexical run through ProximaDB's document Tantivy index.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use proximadb::storage::document::indexes::fulltext::FullTextIndex;
use proximadb_data_model::ProximaValue;
use proximadb_records::{ProximaTree, ProximaTreeNode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

#[derive(Debug, Parser)]
#[command(about = "Score JSONL sources with ProximaDB's document Tantivy projection")]
struct Args {
    #[arg(long)]
    documents: PathBuf,
    #[arg(long)]
    queries: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long, default_value_t = 100)]
    top_k: usize,
    #[arg(long, value_enum, default_value_t = QueryErrorPolicy::Fail)]
    query_error_policy: QueryErrorPolicy,
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum QueryErrorPolicy {
    Fail,
    ZeroPad,
}

#[derive(Debug, Deserialize)]
struct DocumentInput {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Deserialize)]
struct QueryInput {
    id: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct RunRow<'a> {
    query_id: &'a str,
    rank: usize,
    score: f32,
    source_id: &'a str,
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut source = File::open(path)
        .with_context(|| format!("failed to open {} for hashing", path.display()))?;
    let mut digest = Sha256::new();
    std::io::copy(&mut source, &mut digest)
        .with_context(|| format!("failed to hash {}", path.display()))?;
    Ok(format!("{:x}", digest.finalize()))
}

fn load_jsonl<T: DeserializeOwned>(path: &Path, kind: &str) -> Result<Vec<T>> {
    let source = File::open(path).with_context(|| format!("failed to open {kind}"))?;
    let mut values = Vec::new();
    for (index, line) in BufReader::new(source).lines().enumerate() {
        let line = line.with_context(|| format!("failed to read {kind} line {}", index + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        values.push(
            serde_json::from_str(&line)
                .with_context(|| format!("invalid {kind} JSON at line {}", index + 1))?,
        );
    }
    if values.is_empty() {
        bail!("{kind} contains no records");
    }
    Ok(values)
}

fn validate_inputs(
    documents: &[DocumentInput],
    queries: &[QueryInput],
    top_k: usize,
) -> Result<()> {
    if top_k == 0 || top_k > documents.len() {
        bail!("top_k must be in 1..={}", documents.len());
    }
    let mut document_ids = std::collections::HashSet::new();
    for document in documents {
        if document.id.trim().is_empty() || !document_ids.insert(document.id.as_str()) {
            bail!("document IDs must be non-empty and unique");
        }
        if document.title.trim().is_empty() && document.body.trim().is_empty() {
            bail!("document '{}' has no title or body", document.id);
        }
    }
    let mut query_ids = std::collections::HashSet::new();
    for query in queries {
        if query.id.trim().is_empty() || !query_ids.insert(query.id.as_str()) {
            bail!("query IDs must be non-empty and unique");
        }
        if query.text.trim().is_empty() {
            bail!("query '{}' has no text", query.id);
        }
    }
    Ok(())
}

fn document_tree(document: &DocumentInput) -> ProximaTree {
    ProximaTree::from([
        (
            "title".to_string(),
            ProximaTreeNode::Value(ProximaValue::String(document.title.clone())),
        ),
        (
            "body".to_string(),
            ProximaTreeNode::Value(ProximaValue::String(document.body.clone())),
        ),
    ])
}

fn atomic_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let parent = path.parent().context("output path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.flush()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to persist {}", path.display()))?;
    Ok(())
}

fn run(args: &Args) -> Result<serde_json::Value> {
    let documents: Vec<DocumentInput> = load_jsonl(&args.documents, "documents")?;
    let queries: Vec<QueryInput> = load_jsonl(&args.queries, "queries")?;
    validate_inputs(&documents, &queries, args.top_k)?;

    // Reopen after commit so the evaluation observes the same persisted
    // segment boundary used after a process restart, without relying on the
    // asynchronous OnCommitWithDelay reader refresh.
    let index_root = tempfile::tempdir()?;
    {
        let mut index = FullTextIndex::new_persistent("source-eval", index_root.path())?;
        index.add_field("title")?;
        index.add_field("body")?;
        for document in &documents {
            index.index_document(&document.id, &document_tree(document))?;
        }
        index.commit()?;
    }
    let mut index = FullTextIndex::new_persistent("source-eval", index_root.path())?;
    index.add_field("title")?;
    index.add_field("body")?;

    let output_parent = args.output.parent().context("output path has no parent")?;
    std::fs::create_dir_all(output_parent)?;
    let mut temporary = NamedTempFile::new_in(output_parent)?;
    let mut query_errors = Vec::new();
    {
        let mut output = BufWriter::new(&mut temporary);
        for query in &queries {
            let matched: HashMap<String, f32> = match index.search(&query.text, documents.len()) {
                Ok(results) => results.into_iter().collect(),
                Err(error) => match args.query_error_policy {
                    QueryErrorPolicy::Fail => {
                        return Err(error).with_context(|| {
                            format!(
                                "query '{}' failed under strict production parsing",
                                query.id
                            )
                        });
                    }
                    QueryErrorPolicy::ZeroPad => {
                        query_errors.push(json!({
                            "query_id": query.id,
                            "error": format!("{error:#}"),
                        }));
                        HashMap::new()
                    }
                },
            };
            let mut ranked: Vec<(&str, f32)> = documents
                .iter()
                .map(|document| {
                    (
                        document.id.as_str(),
                        matched.get(&document.id).copied().unwrap_or(0.0),
                    )
                })
                .collect();
            if ranked.iter().any(|(_, score)| !score.is_finite()) {
                bail!("Tantivy returned a non-finite score");
            }
            ranked.sort_by(|left, right| {
                right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0))
            });
            for (rank, (source_id, score)) in ranked.into_iter().take(args.top_k).enumerate() {
                serde_json::to_writer(
                    &mut output,
                    &RunRow {
                        query_id: &query.id,
                        rank: rank + 1,
                        score,
                        source_id,
                    },
                )?;
                output.write_all(b"\n")?;
            }
        }
        output.flush()?;
    }
    temporary
        .persist(&args.output)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to persist {}", args.output.display()))?;

    let manifest = json!({
        "schema_version": 1,
        "producer_sha256": sha256_bytes(include_bytes!("score_tantivy_corpus.rs")),
        "engine": {
            "implementation": "proximadb::storage::document::indexes::fulltext::FullTextIndex",
            "library": "tantivy",
            "library_requirement": "0.22",
            "query_parser": "Tantivy QueryParser strict",
            "scoring": "Tantivy BM25 defaults",
            "text_fields": ["title", "body"],
        },
        "candidate_granularity": "source",
        "tie_break": "score descending, source_id ascending",
        "zero_score_policy": "pad the full source universe deterministically before top-k",
        "query_error_policy": args.query_error_policy,
        "query_error_count": query_errors.len(),
        "query_errors": query_errors,
        "top_k": args.top_k,
        "document_count": documents.len(),
        "query_count": queries.len(),
        "run_row_count": queries.len() * args.top_k,
        "documents_path": args.documents.canonicalize()?.display().to_string(),
        "documents_sha256": sha256_file(&args.documents)?,
        "queries_path": args.queries.canonicalize()?.display().to_string(),
        "queries_sha256": sha256_file(&args.queries)?,
        "run_path": args.output.canonicalize()?.display().to_string(),
        "run_sha256": sha256_file(&args.output)?,
        "limitations": [
            "quality evidence only; no serving latency claim",
            "this is the document Tantivy projection, not the canonical hybrid endpoint custom FullTextIndex",
        ],
    });
    atomic_json(&args.manifest, &manifest)?;
    Ok(manifest)
}

fn main() -> Result<()> {
    let manifest = run(&Args::parse())?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_title_matches_and_emits_complete_deterministic_depth() -> Result<()> {
        let root = tempfile::tempdir()?;
        let documents = root.path().join("documents.jsonl");
        let queries = root.path().join("queries.jsonl");
        let output = root.path().join("run.jsonl");
        let manifest_path = root.path().join("manifest.json");
        std::fs::write(
            &documents,
            concat!(
                "{\"id\":\"doc-z\",\"title\":\"needle title\",\"body\":\"plain\"}\n",
                "{\"id\":\"doc-b\",\"title\":\"other\",\"body\":\"plain\"}\n",
                "{\"id\":\"doc-a\",\"title\":\"other\",\"body\":\"plain\"}\n"
            ),
        )?;
        std::fs::write(&queries, "{\"id\":\"q1\",\"text\":\"needle\"}\n")?;

        let manifest = run(&Args {
            documents,
            queries,
            output: output.clone(),
            manifest: manifest_path,
            top_k: 3,
            query_error_policy: QueryErrorPolicy::Fail,
        })?;

        let rows: Vec<serde_json::Value> = load_jsonl(&output, "run")?;
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.get("source_id").and_then(serde_json::Value::as_str))
            .collect();
        assert_eq!(ids, vec!["doc-z", "doc-a", "doc-b"]);
        assert_eq!(manifest["run_row_count"], 3);
        assert_eq!(manifest["engine"]["library"], "tantivy");
        Ok(())
    }

    #[test]
    fn zero_pad_policy_records_strict_query_parser_failures() -> Result<()> {
        let root = tempfile::tempdir()?;
        let documents = root.path().join("documents.jsonl");
        let queries = root.path().join("queries.jsonl");
        let output = root.path().join("run.jsonl");
        std::fs::write(
            &documents,
            concat!(
                "{\"id\":\"doc-b\",\"title\":\"plain\",\"body\":\"text\"}\n",
                "{\"id\":\"doc-a\",\"title\":\"plain\",\"body\":\"text\"}\n"
            ),
        )?;
        std::fs::write(&queries, "{\"id\":\"q-bad\",\"text\":\"unknown:value\"}\n")?;

        let manifest = run(&Args {
            documents,
            queries,
            output: output.clone(),
            manifest: root.path().join("manifest.json"),
            top_k: 2,
            query_error_policy: QueryErrorPolicy::ZeroPad,
        })?;

        let rows: Vec<serde_json::Value> = load_jsonl(&output, "run")?;
        let ids: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.get("source_id").and_then(serde_json::Value::as_str))
            .collect();
        assert_eq!(ids, vec!["doc-a", "doc-b"]);
        assert_eq!(manifest["query_error_count"], 1);
        assert_eq!(manifest["query_errors"][0]["query_id"], "q-bad");
        Ok(())
    }
}
