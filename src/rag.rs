use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use crate::db::{self, ChatFile, KnowledgeBaseFile, MessageCitation, RagChunk};
use crate::providers::ProviderError;

pub const SHORT_TEXT_INLINE_LIMIT: usize = 4_000;
const CHUNK_SIZE: usize = 1_600;
const CHUNK_OVERLAP: usize = 240;
const FALLBACK_DIMENSIONS: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievedChunk {
    pub source_label: String,
    pub path: String,
    pub content: String,
    pub score: f32,
}

#[derive(Debug, Deserialize)]
struct OllamaEmbedResponse {
    #[serde(default)]
    embeddings: Vec<Vec<f32>>,
    embedding: Vec<f32>,
}

pub fn default_embedding_models() -> Vec<String> {
    vec![
        "nomic-embed-text".to_string(),
        "mxbai-embed-large".to_string(),
        "all-minilm".to_string(),
    ]
}

pub fn normalize_upload_path(filename: &str) -> String {
    format!("/uploads/{}", sanitize_name(filename))
}

pub fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches('/').trim_matches('_');
    if out.is_empty() {
        "file".to_string()
    } else {
        out.to_string()
    }
}

pub fn inline_context_for_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= SHORT_TEXT_INLINE_LIMIT {
        trimmed.to_string()
    } else {
        String::new()
    }
}

pub fn extract_text(bytes: &[u8], mime_type: &str) -> Option<String> {
    let mime = mime_type.to_ascii_lowercase();
    let looks_textual = mime.starts_with("text/")
        || mime.contains("json")
        || mime.contains("xml")
        || mime.contains("yaml")
        || mime.contains("javascript")
        || mime.contains("markdown")
        || mime.contains("csv")
        || mime.is_empty();

    if !looks_textual && bytes.iter().filter(|b| **b == 0).count() > 0 {
        return None;
    }

    let text = String::from_utf8(bytes.to_vec()).ok()?;
    let printable = text
        .chars()
        .filter(|ch| ch.is_ascii_graphic() || ch.is_ascii_whitespace() || !ch.is_control())
        .count();
    let total = text.chars().count().max(1);
    if printable * 100 / total < 85 {
        return None;
    }
    Some(text)
}

pub fn chunk_text(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + CHUNK_SIZE).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        let chunk = chunk.trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(CHUNK_OVERLAP);
    }
    chunks
}

pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub fn term_frequency(tokens: &[String]) -> HashMap<String, u32> {
    let mut freq = HashMap::new();
    for token in tokens {
        *freq.entry(token.clone()).or_insert(0) += 1;
    }
    freq
}

pub async fn embed_texts(
    base_url: &str,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, ProviderError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    if model.trim().is_empty() {
        return Ok(texts.iter().map(|text| fallback_embedding(text)).collect());
    }

    let response = Client::new()
        .post(format!("{}/api/embed", base_url.trim_end_matches('/')))
        .json(&serde_json::json!({
            "model": model,
            "input": texts,
        }))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            let payload: OllamaEmbedResponse = resp
                .json()
                .await
                .map_err(|err| ProviderError::Parse(err.to_string()))?;
            if !payload.embeddings.is_empty() {
                Ok(payload.embeddings)
            } else if !payload.embedding.is_empty() {
                Ok(vec![payload.embedding])
            } else {
                Ok(texts.iter().map(|text| fallback_embedding(text)).collect())
            }
        }
        Ok(_) | Err(_) => Ok(texts.iter().map(|text| fallback_embedding(text)).collect()),
    }
}

pub async fn index_chat_file(
    conn: &rusqlite::Connection,
    base_url: &str,
    embedding_model: &str,
    file: &ChatFile,
) -> Result<(), ProviderError> {
    if !file.is_text {
        return Ok(());
    }
    let text =
        db::read_storage_text(&file.file_path).map_err(|err| ProviderError::Io(err.to_string()))?;
    index_text_chunks(
        conn,
        base_url,
        embedding_model,
        Some(&file.chat_id),
        None,
        &file.id,
        &file.display_name,
        &file.path,
        &text,
    )
    .await
}

pub async fn index_knowledge_base_file(
    conn: &rusqlite::Connection,
    base_url: &str,
    embedding_model: &str,
    file: &KnowledgeBaseFile,
) -> Result<(), ProviderError> {
    if !file.is_text {
        return Ok(());
    }
    let text =
        db::read_storage_text(&file.file_path).map_err(|err| ProviderError::Io(err.to_string()))?;
    index_text_chunks(
        conn,
        base_url,
        embedding_model,
        None,
        Some(&file.knowledge_base_id),
        &file.id,
        &file.display_name,
        &file.path,
        &text,
    )
    .await
}

async fn index_text_chunks(
    conn: &rusqlite::Connection,
    base_url: &str,
    embedding_model: &str,
    chat_id: Option<&str>,
    knowledge_base_id: Option<&str>,
    file_id: &str,
    file_name: &str,
    path: &str,
    text: &str,
) -> Result<(), ProviderError> {
    db::clear_rag_chunks_for_file(conn, file_id)
        .map_err(|err| ProviderError::Io(err.to_string()))?;
    let chunks = chunk_text(text);
    if chunks.is_empty() {
        return Ok(());
    }

    let embeddings = embed_texts(base_url, embedding_model, &chunks).await?;
    for (index, chunk) in chunks.iter().enumerate() {
        let tokens = tokenize(chunk);
        let term_freq = term_frequency(&tokens);
        let embedding = embeddings
            .get(index)
            .cloned()
            .unwrap_or_else(|| fallback_embedding(chunk));
        db::add_rag_chunk(
            conn,
            chat_id,
            knowledge_base_id,
            file_id,
            file_name,
            path,
            index as i64,
            chunk,
            &serde_json::to_string(&term_freq).unwrap_or_else(|_| "{}".to_string()),
            tokens.len() as i64,
            embedding_model,
            &serde_json::to_string(&embedding).unwrap_or_else(|_| "[]".to_string()),
        )
        .map_err(|err| ProviderError::Io(err.to_string()))?;
    }
    Ok(())
}

pub async fn retrieve_for_chat(
    conn: &rusqlite::Connection,
    base_url: &str,
    chat_id: &str,
    embedding_model: &str,
    query: &str,
    top_k: usize,
) -> Result<Vec<RetrievedChunk>, ProviderError> {
    let mut chunks = db::list_chat_rag_chunks(conn, chat_id)
        .map_err(|err| ProviderError::Io(err.to_string()))?;
    for knowledge_base in db::list_chat_knowledge_bases(conn, chat_id)
        .map_err(|err| ProviderError::Io(err.to_string()))?
    {
        chunks.extend(
            db::list_knowledge_base_rag_chunks(conn, &knowledge_base.id)
                .map_err(|err| ProviderError::Io(err.to_string()))?,
        );
    }
    if chunks.is_empty() {
        return Ok(Vec::new());
    }

    let query_tokens = tokenize(query);
    let query_embedding = embed_texts(base_url, embedding_model, &[query.to_string()])
        .await?
        .into_iter()
        .next()
        .unwrap_or_else(|| fallback_embedding(query));

    let mut document_frequency: HashMap<String, usize> = HashMap::new();
    let mut avg_doc_len = 0.0f32;
    for chunk in &chunks {
        avg_doc_len += chunk.term_count as f32;
        let tf = parse_term_freq(&chunk.term_freq_json);
        let seen: HashSet<_> = tf.keys().cloned().collect();
        for token in seen {
            *document_frequency.entry(token).or_insert(0) += 1;
        }
    }
    avg_doc_len /= chunks.len().max(1) as f32;

    let document_count = chunks.len();
    let mut scored: Vec<RetrievedChunk> = chunks
        .into_iter()
        .map(|chunk| {
            let bm25 = bm25_score(
                &chunk,
                &query_tokens,
                &document_frequency,
                avg_doc_len,
                document_count,
            );
            let vector =
                if chunk.embedding_model == embedding_model || embedding_model.trim().is_empty() {
                    cosine_similarity(&parse_embedding(&chunk.embedding_json), &query_embedding)
                } else {
                    0.0
                };
            let score = (bm25 * 0.65) + (vector * 0.35);
            RetrievedChunk {
                source_label: chunk.file_name.clone(),
                path: chunk.path.clone(),
                content: chunk.content.clone(),
                score,
            }
        })
        .collect();

    scored.sort_by(|left, right| right.score.total_cmp(&left.score));
    scored.retain(|chunk| chunk.score > 0.02);
    scored.truncate(top_k);
    Ok(scored)
}

pub fn format_retrieved_context(chunks: &[RetrievedChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push("Use the retrieved knowledge below when it is relevant. Prefer these sources over guessing, and cite the file name in your answer when you rely on them.".to_string());
    for chunk in chunks {
        lines.push(format!(
            "[Source: {} at {}]\n{}",
            chunk.source_label, chunk.path, chunk.content
        ));
    }
    lines.join("\n\n")
}

pub fn to_message_citations(chunks: &[RetrievedChunk]) -> Vec<MessageCitation> {
    chunks
        .iter()
        .map(|chunk| MessageCitation {
            source_label: chunk.source_label.clone(),
            path: chunk.path.clone(),
            excerpt: citation_excerpt(&chunk.content),
            score: chunk.score,
        })
        .collect()
}

fn citation_excerpt(content: &str) -> String {
    let trimmed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut excerpt = String::new();
    for ch in trimmed.chars().take(220) {
        excerpt.push(ch);
    }
    if trimmed.chars().count() > 220 {
        excerpt.push_str("...");
    }
    excerpt
}

fn parse_term_freq(json: &str) -> HashMap<String, u32> {
    serde_json::from_str(json).unwrap_or_default()
}

fn parse_embedding(json: &str) -> Vec<f32> {
    serde_json::from_str(json).unwrap_or_default()
}

fn bm25_score(
    chunk: &RagChunk,
    query_tokens: &[String],
    document_frequency: &HashMap<String, usize>,
    avg_doc_len: f32,
    document_count: usize,
) -> f32 {
    let tf = parse_term_freq(&chunk.term_freq_json);
    let mut score = 0.0f32;
    let k1 = 1.5f32;
    let b = 0.75f32;
    let doc_len = chunk.term_count as f32;
    let norm = k1 * (1.0 - b + b * (doc_len / avg_doc_len.max(1.0)));

    for token in query_tokens {
        let freq = *tf.get(token).unwrap_or(&0) as f32;
        if freq == 0.0 {
            continue;
        }
        let df = *document_frequency.get(token).unwrap_or(&0) as f32;
        let idf = (((document_count as f32 - df + 0.5) / (df + 0.5)) + 1.0).ln();
        score += idf * ((freq * (k1 + 1.0)) / (freq + norm));
    }
    score
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let len = left.len().min(right.len());
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for index in 0..len {
        dot += left[index] * right[index];
        left_norm += left[index] * left[index];
        right_norm += right[index] * right[index];
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

fn fallback_embedding(text: &str) -> Vec<f32> {
    let mut vector = vec![0.0f32; FALLBACK_DIMENSIONS];
    for token in tokenize(text) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hasher);
        let hash = hasher.finish() as usize;
        let index = hash % FALLBACK_DIMENSIONS;
        vector[index] += 1.0;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}
