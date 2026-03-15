// TantivyStore: persistent segment storage using Tantivy
// Phase 1.5 — replaces sled KV store
//
// Tantivy 0.22 API notes:
//   • `TantivyDocument` is the concrete document struct (renamed from old `Document`)
//   • `Document` is the trait
//   • `OwnedValue` carries field values; use pattern matching (no `as_str`/`as_bytes` helpers)
//   • `searcher.doc::<TantivyDocument>(addr)?` — explicit type parameter required

use crate::error::{Result, StoreError};
use keel_proto::pb::MemoryChunk;
use prost::Message;
use std::path::Path;
use std::sync::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::{QueryParser, TermQuery};
use tantivy::schema::{BytesOptions, Field, NumericOptions, OwnedValue, Schema, STRING, STORED, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

const EMBEDDING_TOP_K: usize = 20;

/// Quantize a float embedding to discrete Tantivy term tokens.
///
/// Top-K dimensions by absolute magnitude are encoded as:
///   "ep{i}" — dimension i is positive
///   "en{i}" — dimension i is negative
pub fn quantize_embedding(embedding: &[f32]) -> String {
    let mut indexed: Vec<(usize, f32)> = embedding
        .iter()
        .enumerate()
        .map(|(i, &v)| (i, v))
        .collect();
    indexed.sort_by(|a, b| {
        b.1.abs()
            .partial_cmp(&a.1.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    indexed.truncate(EMBEDDING_TOP_K);
    indexed
        .iter()
        .map(|(i, v)| {
            if *v >= 0.0 {
                format!("ep{}", i)
            } else {
                format!("en{}", i)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct TantivyStore {
    index: Index,
    writer: Mutex<IndexWriter>,
    reader: IndexReader,
    f_id: Field,
    f_session_id: Field,
    f_payload_text: Field,
    f_embedding_terms: Field,
    f_created_at_ms: Field,
    f_ttl_ms: Field,
    f_raw_bytes: Field,
}

impl TantivyStore {
    pub fn new(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = Path::new(data_dir);

        let mut builder = Schema::builder();
        let _ = builder.add_text_field("id", STRING | STORED);
        let _ = builder.add_text_field("session_id", STRING | STORED);
        let _ = builder.add_text_field("payload_text", TEXT | STORED);
        let _ = builder.add_text_field("embedding_terms", TEXT);
        let _ = builder.add_u64_field("created_at_ms", NumericOptions::default().set_stored());
        let _ = builder.add_u64_field("ttl_ms", NumericOptions::default().set_stored());
        let _ = builder.add_bytes_field("raw_bytes", BytesOptions::default().set_stored());
        let schema = builder.build();

        let index = if path.join("meta.json").exists() {
            Index::open_in_dir(path).map_err(|e| StoreError::Storage(e.to_string()))?
        } else {
            Index::create_in_dir(path, schema).map_err(|e| StoreError::Storage(e.to_string()))?
        };

        let live = index.schema();
        let get = |name: &str| {
            live.get_field(name)
                .map_err(|e| StoreError::Storage(e.to_string()))
        };
        let f_id = get("id")?;
        let f_session_id = get("session_id")?;
        let f_payload_text = get("payload_text")?;
        let f_embedding_terms = get("embedding_terms")?;
        let f_created_at_ms = get("created_at_ms")?;
        let f_ttl_ms = get("ttl_ms")?;
        let f_raw_bytes = get("raw_bytes")?;

        let writer = index
            .writer(50_000_000)
            .map_err(|e| StoreError::Storage(e.to_string()))?;

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e: tantivy::TantivyError| StoreError::Storage(e.to_string()))?;

        Ok(Self {
            index,
            writer: Mutex::new(writer),
            reader,
            f_id,
            f_session_id,
            f_payload_text,
            f_embedding_terms,
            f_created_at_ms,
            f_ttl_ms,
            f_raw_bytes,
        })
    }

    pub fn write(&self, chunk: &MemoryChunk, embedding_terms: &str) -> Result<()> {
        let payload_text = String::from_utf8_lossy(&chunk.payload).into_owned();
        let raw_bytes = chunk.encode_to_vec();

        let mut doc = TantivyDocument::default();
        doc.add_text(self.f_id, &chunk.id);
        doc.add_text(self.f_session_id, &chunk.session_id);
        doc.add_text(self.f_payload_text, &payload_text);
        if !embedding_terms.is_empty() {
            doc.add_text(self.f_embedding_terms, embedding_terms);
        }
        doc.add_u64(self.f_created_at_ms, chunk.created_at_ms);
        doc.add_u64(self.f_ttl_ms, chunk.ttl_ms);
        doc.add_bytes(self.f_raw_bytes, raw_bytes);

        let mut writer = self.writer.lock().unwrap();
        writer
            .add_document(doc)
            .map_err(|e| StoreError::Storage(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| StoreError::Storage(e.to_string()))?;
        self.reader
            .reload()
            .map_err(|e| StoreError::Storage(e.to_string()))?;
        Ok(())
    }

    pub fn read(&self, id: &str) -> Result<Option<MemoryChunk>> {
        let searcher = self.reader.searcher();
        let term = Term::from_field_text(self.f_id, id);
        let query = TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| StoreError::Storage(e.to_string()))?;

        if let Some((_score, addr)) = top_docs.first() {
            let doc: TantivyDocument = searcher
                .doc(*addr)
                .map_err(|e| StoreError::Storage(e.to_string()))?;
            if let Some(bytes) = doc.get_first(self.f_raw_bytes).and_then(owned_as_bytes) {
                let chunk = MemoryChunk::decode(bytes)?;
                return Ok(Some(chunk));
            }
        }
        Ok(None)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let term = Term::from_field_text(self.f_id, id);
        let mut writer = self.writer.lock().unwrap();
        writer.delete_term(term);
        writer
            .commit()
            .map_err(|e| StoreError::Storage(e.to_string()))?;
        self.reader
            .reload()
            .map_err(|e| StoreError::Storage(e.to_string()))?;
        Ok(())
    }

    pub fn search_by_text(&self, text: &str, top_k: usize) -> Result<Vec<String>> {
        let parser = QueryParser::for_index(&self.index, vec![self.f_payload_text]);
        let query = parser
            .parse_query(text)
            .map_err(|e| StoreError::Storage(e.to_string()))?;
        self.collect_ids(&*query, top_k)
    }

    pub fn search_by_embedding_terms(&self, terms: &str, top_k: usize) -> Result<Vec<String>> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let parser = QueryParser::for_index(&self.index, vec![self.f_embedding_terms]);
        let query = match parser.parse_query(terms) {
            Ok(q) => q,
            Err(_) => return Ok(Vec::new()),
        };
        self.collect_ids(&*query, top_k)
    }

    pub fn ids_for_session(&self, session_id: &str) -> Result<Vec<String>> {
        let term = Term::from_field_text(self.f_session_id, session_id);
        let query = TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
        self.collect_ids(&query, 1_000_000)
    }

    pub fn count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    fn collect_ids(
        &self,
        query: &dyn tantivy::query::Query,
        limit: usize,
    ) -> Result<Vec<String>> {
        let searcher = self.reader.searcher();
        let top_docs = searcher
            .search(query, &TopDocs::with_limit(limit))
            .map_err(|e| StoreError::Storage(e.to_string()))?;

        let mut ids = Vec::new();
        for (_score, addr) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(addr)
                .map_err(|e| StoreError::Storage(e.to_string()))?;
            if let Some(id) = doc.get_first(self.f_id).and_then(owned_as_str) {
                ids.push(id.to_string());
            }
        }
        Ok(ids)
    }
}

fn owned_as_bytes(v: &OwnedValue) -> Option<&[u8]> {
    match v {
        OwnedValue::Bytes(b) => Some(b.as_slice()),
        _ => None,
    }
}

fn owned_as_str(v: &OwnedValue) -> Option<&str> {
    match v {
        OwnedValue::Str(s) => Some(s.as_str()),
        _ => None,
    }
}
