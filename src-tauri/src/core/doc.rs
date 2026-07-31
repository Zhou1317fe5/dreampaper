use std::io::Read;
use std::path::Path;

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use uuid::Uuid;
use zip::ZipArchive;

use crate::error::{AppError, AppResult};

use super::store::Store;

const MAX_SOURCE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_INDEX_CHARS: usize = 300_000;
const CHUNK_CHARS: usize = 2_000;
const CHUNK_OVERLAP_CHARS: usize = 200;

#[derive(Clone, Debug, Serialize)]
pub struct DocumentSummary {
    pub id: String,
    pub title: Option<String>,
    pub parser: String,
    pub status: String,
    pub chunk_count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DocumentChunkHit {
    pub id: String,
    pub document_id: String,
    pub heading: Option<String>,
    pub page: Option<i64>,
    pub text: String,
    pub score: f64,
}

pub struct DocumentService<'a> {
    store: &'a Store,
    app_data: &'a Path,
}

impl<'a> DocumentService<'a> {
    pub fn new(store: &'a Store, app_data: &'a Path) -> Self {
        Self { store, app_data }
    }

    pub fn import_document(&self, path: String) -> AppResult<DocumentSummary> {
        let source = Path::new(&path);
        if !source.exists() {
            return Err(AppError::new(
                "document_not_found",
                "Document path does not exist",
            ));
        }
        let metadata = std::fs::metadata(source)?;
        if metadata.len() > MAX_SOURCE_BYTES {
            return Err(AppError::with_detail(
                "document_too_large",
                "Document exceeds the lightweight desktop import limit",
                serde_json::json!({
                    "max_bytes": MAX_SOURCE_BYTES,
                    "actual_bytes": metadata.len(),
                }),
            ));
        }

        std::fs::create_dir_all(self.app_data.join("uploads"))?;
        let doc_id = Uuid::new_v4().to_string();
        let title = source
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string);
        let parser = parser_name(source);
        let parsed = extract_text(source, parser)?;
        let text = truncate_chars(&parsed, MAX_INDEX_CHARS);
        let status = if text.trim().is_empty() {
            "metadata_only"
        } else if parsed.chars().count() > MAX_INDEX_CHARS {
            "indexed_truncated"
        } else {
            "indexed"
        };
        let now = Utc::now().to_rfc3339();
        let conn = self.store.connection()?;
        conn.execute(
            "INSERT INTO documents(id, title, parser, status, meta_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                doc_id,
                title,
                parser,
                status,
                serde_json::json!({
                    "source_path": source.to_string_lossy(),
                    "bytes": metadata.len(),
                    "max_index_chars": MAX_INDEX_CHARS,
                })
                .to_string(),
                now
            ],
        )?;

        let chunks = chunk_text(if text.trim().is_empty() {
            title.as_deref().unwrap_or_default()
        } else {
            &text
        });
        for (index, chunk) in chunks.iter().enumerate() {
            let chunk_id = Uuid::new_v4().to_string();
            let start_char = chunk.start as i64;
            let end_char = chunk.end as i64;
            conn.execute(
                "INSERT INTO chunks(id, document_id, ord, heading, page, start_char, end_char, text, meta_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    chunk_id,
                    doc_id,
                    index as i64,
                    title,
                    Option::<i64>::None,
                    start_char,
                    end_char,
                    chunk.text,
                    "{}"
                ],
            )?;
            conn.execute(
                "INSERT INTO chunks_fts(rowid, title, heading, body) VALUES ((SELECT rowid FROM chunks WHERE id = ?1), ?2, ?3, ?4)",
                params![chunk_id, title, title, chunk.text],
            )?;
        }
        Ok(DocumentSummary {
            id: doc_id,
            title,
            parser: parser.to_string(),
            status: status.to_string(),
            chunk_count: chunks.len() as i64,
        })
    }

    pub fn search_documents(
        &self,
        query: String,
        limit: usize,
    ) -> AppResult<Vec<DocumentChunkHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.store.connection()?;
        let safe_query = fts_query(&query);
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            "SELECT chunks.id, chunks.document_id, chunks.heading, chunks.page, chunks.text, bm25(chunks_fts) AS score \
             FROM chunks_fts JOIN chunks ON chunks_fts.rowid = chunks.rowid \
             WHERE chunks_fts MATCH ?1 ORDER BY score LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![safe_query, limit.clamp(1, 50) as i64], |row| {
            Ok(DocumentChunkHit {
                id: row.get(0)?,
                document_id: row.get(1)?,
                heading: row.get(2)?,
                page: row.get(3)?,
                text: row.get(4)?,
                score: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

struct TextChunk {
    start: usize,
    end: usize,
    text: String,
}

fn extract_text(path: &Path, parser: &str) -> AppResult<String> {
    match parser {
        "text" | "markdown" | "csv" | "json" => Ok(std::fs::read_to_string(path).unwrap_or_else(|_| String::new())),
        "docx" => extract_docx_text(path),
        "pdf" => Ok(format!(
            "PDF file imported as metadata only: {}. Full PDF text extraction is intentionally deferred in the lightweight desktop core.",
            path.file_name().and_then(|value| value.to_str()).unwrap_or("document.pdf")
        )),
        _ => Ok(path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string()),
    }
}

fn extract_docx_text(path: &Path) -> AppResult<String> {
    let file = std::fs::File::open(path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| AppError::new("docx_parse_error", error.to_string()))?;
    let mut document = archive
        .by_name("word/document.xml")
        .map_err(|error| AppError::new("docx_parse_error", error.to_string()))?;
    let mut xml = String::new();
    document.read_to_string(&mut xml)?;
    Ok(extract_xml_text(&xml))
}

fn extract_xml_text(xml: &str) -> String {
    let mut output = String::new();
    let mut in_text = false;
    let mut current = String::new();
    let mut chars = xml.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            let mut tag = String::new();
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
                tag.push(next);
            }
            let name = tag.split_whitespace().next().unwrap_or_default();
            match name {
                "w:t" => {
                    in_text = true;
                    current.clear();
                }
                "/w:t" => {
                    in_text = false;
                    output.push_str(&decode_entities(&current));
                }
                "w:tab" | "w:tab/" => output.push('\t'),
                "w:br" | "w:br/" | "/w:p" => output.push('\n'),
                _ => {}
            }
        } else if in_text {
            current.push(ch);
        }
    }
    normalize_text(&output)
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn normalize_text(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn chunk_text(value: &str) -> Vec<TextChunk> {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return vec![TextChunk {
            start: 0,
            end: 0,
            text: String::new(),
        }];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + CHUNK_CHARS).min(chars.len());
        let text = chars[start..end].iter().collect::<String>();
        chunks.push(TextChunk { start, end, text });
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP_CHARS);
    }
    chunks
}

fn fts_query(query: &str) -> String {
    query
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
        .filter(|part| !part.is_empty())
        .take(12)
        .map(|part| format!("\"{}\"", part.replace('"', " ")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn parser_name(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" => "text",
        "md" | "markdown" => "markdown",
        "csv" | "tsv" => "csv",
        "json" => "json",
        "docx" => "docx",
        "pdf" => "pdf",
        _ => "metadata-only",
    }
}
