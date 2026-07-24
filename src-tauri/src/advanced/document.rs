use std::io::{Cursor, Read};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

use crate::workbench;

const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentContent {
    pub path: String,
    pub name: String,
    pub media_type: String,
    pub content: String,
    pub source_bytes: u64,
    pub extracted_bytes: usize,
    pub truncated: bool,
}

pub fn extract_document(root: &Path, relative: &str) -> Result<DocumentContent, String> {
    let path = workbench::resolve_workspace_path(root, relative, false)
        .map_err(|error| error.to_string())?;
    let metadata = path.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_DOCUMENT_BYTES {
        return Err("document must be a file no larger than 8 MiB".into());
    }
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (content, media_type) = match extension.as_str() {
        "pdf" => (
            pdf_extract::extract_text_from_mem(&bytes)
                .map_err(|error| format!("PDF extraction failed: {error}"))?,
            "application/pdf",
        ),
        "docx" => (
            extract_docx(&bytes)?,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        extension if is_plain_text(extension) => (
            String::from_utf8(bytes).map_err(|_| "text document is not valid UTF-8")?,
            "text/plain",
        ),
        _ => {
            return Err(
                "unsupported document type; use text, Markdown, JSON, CSV, PDF, or DOCX".into(),
            );
        }
    };
    let (content, truncated) = bound_utf8(content, MAX_EXTRACTED_BYTES);
    Ok(DocumentContent {
        path: relative.replace('\\', "/"),
        name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(relative)
            .to_string(),
        media_type: media_type.into(),
        extracted_bytes: content.len(),
        content,
        source_bytes: metadata.len(),
        truncated,
    })
}

fn extract_docx(bytes: &[u8]) -> Result<String, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("DOCX archive is invalid: {error}"))?;
    let mut document = archive
        .by_name("word/document.xml")
        .map_err(|error| format!("DOCX document.xml is missing: {error}"))?;
    if document.size() > MAX_DOCUMENT_BYTES {
        return Err("DOCX document XML exceeds the 8 MiB limit".into());
    }
    let mut xml = Vec::with_capacity(document.size() as usize);
    document
        .read_to_end(&mut xml)
        .map_err(|error| error.to_string())?;
    let mut reader = Reader::from_reader(xml.as_slice());
    reader.config_mut().trim_text(true);
    let mut output = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => {
                let decoded = text.decode().map_err(|error| error.to_string())?;
                if !output.is_empty() && !output.ends_with([' ', '\n']) {
                    output.push(' ');
                }
                output.push_str(&decoded);
                if output.len() >= MAX_EXTRACTED_BYTES {
                    break;
                }
            }
            Ok(Event::End(end)) if end.name().as_ref() == b"w:p" => output.push('\n'),
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("DOCX XML is invalid: {error}")),
        }
    }
    Ok(output)
}

fn is_plain_text(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "md"
            | "markdown"
            | "json"
            | "csv"
            | "tsv"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "html"
            | "log"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "cs"
            | "sql"
    )
}

fn bound_utf8(mut value: String, max: usize) -> (String, bool) {
    if value.len() <= max {
        return (value, false);
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    (value, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_bounded_text_and_rejects_unknown_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "# Notes\nUse pnpm").unwrap();
        std::fs::write(dir.path().join("archive.bin"), [0, 1, 2]).unwrap();
        let document = extract_document(dir.path(), "notes.md").unwrap();
        assert!(document.content.contains("Use pnpm"));
        assert!(extract_document(dir.path(), "archive.bin").is_err());
    }
}
