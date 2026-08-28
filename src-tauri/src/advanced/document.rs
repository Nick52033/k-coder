use std::borrow::Cow;
use std::io::{Cursor, Read};
use std::path::Path;

use base64::Engine;
use calamine::{Data, Reader};
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::workbench;

const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: usize = 512 * 1024;
const MAX_SPREADSHEET_ARCHIVE_ENTRIES: usize = 4096;
const MAX_SPREADSHEET_ARCHIVE_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SPREADSHEET_ARCHIVE_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;

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
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(relative)
        .to_string();
    extract_document_bytes(relative.replace('\\', "/"), name, bytes)
}

pub fn extract_document_data_url(name: &str, data_url: &str) -> Result<DocumentContent, String> {
    let name = validate_memory_document_name(name)?;
    let (_, encoded) = data_url
        .split_once(',')
        .filter(|(metadata, _)| {
            metadata.len() <= 255
                && metadata.starts_with("data:")
                && metadata.ends_with(";base64")
                && metadata.chars().all(|value| value.is_ascii_graphic())
        })
        .ok_or("document attachment must be a base64 data URL")?;
    let max_encoded_bytes = ((MAX_DOCUMENT_BYTES as usize + 2) / 3) * 4;
    if encoded.len() > max_encoded_bytes {
        return Err("document must be no larger than 8 MiB".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "document attachment is not valid base64")?;
    if bytes.len() > MAX_DOCUMENT_BYTES as usize {
        return Err("document must be no larger than 8 MiB".into());
    }
    let fingerprint =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&Sha256::digest(&bytes)[..12]);
    extract_document_bytes(format!("attachment://{fingerprint}/{name}"), name, bytes)
}

fn extract_document_bytes(
    path: String,
    name: String,
    bytes: Vec<u8>,
) -> Result<DocumentContent, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES as usize {
        return Err("document must be no larger than 8 MiB".into());
    }
    let extension = Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let source_bytes = bytes.len() as u64;
    let (content, media_type, parser_truncated) = match extension.as_str() {
        "pdf" => (
            pdf_extract::extract_text_from_mem(&bytes)
                .map_err(|error| format!("PDF extraction failed: {error}"))?,
            "application/pdf",
            false,
        ),
        "docx" => (
            extract_docx(&bytes)?,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            false,
        ),
        extension if is_spreadsheet(extension) => {
            let (content, truncated) = extract_spreadsheet(&bytes, extension)?;
            (content, spreadsheet_media_type(extension), truncated)
        }
        extension if is_plain_text(extension) => (
            String::from_utf8(bytes).map_err(|_| "text document is not valid UTF-8")?,
            "text/plain",
            false,
        ),
        _ => {
            return Err(
                "unsupported document type; use text, Markdown, JSON, CSV, PDF, DOCX, or Excel"
                    .into(),
            );
        }
    };
    let (content, bound_truncated) = bound_utf8(content, MAX_EXTRACTED_BYTES);
    Ok(DocumentContent {
        path,
        name,
        media_type: media_type.into(),
        extracted_bytes: content.len(),
        content,
        source_bytes,
        truncated: parser_truncated || bound_truncated,
    })
}

fn validate_memory_document_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty()
        || name.chars().count() > 255
        || name.chars().any(char::is_control)
        || name.contains(['/', '\\'])
        || matches!(name, "." | "..")
    {
        return Err("document attachment name is invalid".into());
    }
    Ok(name.to_string())
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
    let mut reader = XmlReader::from_reader(xml.as_slice());
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

fn extract_spreadsheet(bytes: &[u8], extension: &str) -> Result<(String, bool), String> {
    if matches!(extension, "xlsx" | "xlsm" | "xlsb") {
        validate_spreadsheet_archive(bytes)?;
    }

    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|error| format!("Excel extraction failed: {error}"))?;
    let sheet_names = workbook.sheet_names().to_vec();
    let mut output = String::new();

    for (sheet_index, sheet_name) in sheet_names.iter().enumerate() {
        if sheet_index > 0 && !append_bounded(&mut output, "\n", MAX_EXTRACTED_BYTES) {
            return Ok((output, true));
        }
        let header = format!("[工作表: {sheet_name}]\n");
        if !append_bounded(&mut output, &header, MAX_EXTRACTED_BYTES) {
            return Ok((output, true));
        }
        let range = workbook
            .worksheet_range(sheet_name)
            .map_err(|error| format!("Excel sheet '{sheet_name}' extraction failed: {error}"))?;
        for row in range.rows() {
            for (cell_index, cell) in row.iter().enumerate() {
                if cell_index > 0 && !append_bounded(&mut output, "\t", MAX_EXTRACTED_BYTES) {
                    return Ok((output, true));
                }
                if !append_tsv_cell(&mut output, cell, MAX_EXTRACTED_BYTES) {
                    return Ok((output, true));
                }
            }
            if !append_bounded(&mut output, "\n", MAX_EXTRACTED_BYTES) {
                return Ok((output, true));
            }
        }
    }

    Ok((output, false))
}

fn validate_spreadsheet_archive(bytes: &[u8]) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Excel archive is invalid: {error}"))?;
    validate_spreadsheet_archive_entry_count(archive.len())?;

    let mut entry_sizes = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| format!("Excel archive entry is invalid: {error}"))?;
        entry_sizes.push(entry.size());
    }
    validate_spreadsheet_archive_sizes(&entry_sizes)
}

fn validate_spreadsheet_archive_entry_count(entry_count: usize) -> Result<(), String> {
    if entry_count > MAX_SPREADSHEET_ARCHIVE_ENTRIES {
        return Err(format!(
            "Excel archive contains more than {MAX_SPREADSHEET_ARCHIVE_ENTRIES} entries"
        ));
    }
    Ok(())
}

fn validate_spreadsheet_archive_sizes(entry_sizes: &[u64]) -> Result<(), String> {
    let mut expanded_bytes = 0_u64;
    for entry_size in entry_sizes {
        if *entry_size > MAX_SPREADSHEET_ARCHIVE_ENTRY_BYTES {
            return Err("Excel archive entry exceeds the 16 MiB expanded limit".into());
        }
        expanded_bytes = expanded_bytes
            .checked_add(*entry_size)
            .ok_or("Excel archive expanded size overflow")?;
        if expanded_bytes > MAX_SPREADSHEET_ARCHIVE_EXPANDED_BYTES {
            return Err("Excel archive exceeds the 64 MiB total expanded limit".into());
        }
    }
    Ok(())
}

fn append_tsv_cell(output: &mut String, cell: &Data, max: usize) -> bool {
    let displayed = match cell {
        Data::Empty => Cow::Borrowed(""),
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => {
            Cow::Borrowed(value.as_str())
        }
        _ => Cow::Owned(cell.to_string()),
    };
    for character in displayed.chars() {
        let escaped = match character {
            '\\' => "\\\\",
            '\t' => "\\t",
            '\r' => "\\r",
            '\n' => "\\n",
            _ => {
                let mut encoded = [0_u8; 4];
                if !append_bounded(output, character.encode_utf8(&mut encoded), max) {
                    return false;
                }
                continue;
            }
        };
        if !append_bounded(output, escaped, max) {
            return false;
        }
    }
    true
}

fn append_bounded(output: &mut String, value: &str, max: usize) -> bool {
    let remaining = max.saturating_sub(output.len());
    if value.len() <= remaining {
        output.push_str(value);
        return true;
    }

    let mut end = remaining;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&value[..end]);
    false
}

fn is_spreadsheet(extension: &str) -> bool {
    matches!(extension, "xlsx" | "xls" | "xlsm" | "xlsb")
}

fn spreadsheet_media_type(extension: &str) -> &'static str {
    match extension {
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "xlsm" => "application/vnd.ms-excel.sheet.macroEnabled.12",
        "xlsb" => "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
        _ => "application/octet-stream",
    }
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
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hpp"
            | "swift"
            | "kt"
            | "kts"
            | "rb"
            | "php"
            | "scala"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "psm1"
            | "bat"
            | "cmd"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "vue"
            | "svelte"
            | "astro"
            | "ini"
            | "cfg"
            | "conf"
            | "properties"
            | "gradle"
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
    use std::io::Write;

    fn test_xlsx() -> Vec<u8> {
        let files = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="预算" sheetId="1" r:id="rId1"/>
    <sheet name="备注" sheetId="2" r:id="rId2"/>
  </sheets>
</workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
</Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
    <row r="1"><c r="A1" t="inlineStr"><is><t>项目</t></is></c><c r="B1" t="inlineStr"><is><t>金额</t></is></c></row>
    <row r="2"><c r="A2" t="inlineStr"><is><t>住宿</t></is></c><c r="B2"><v>128.5</v></c></row>
  </sheetData>
</worksheet>"#,
            ),
            (
                "xl/worksheets/sheet2.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData>
    <row r="1"><c r="A1" t="inlineStr"><is><t xml:space="preserve">第一行&#10;第二行&#9;尾</t></is></c></row>
  </sheetData>
</worksheet>"#,
            ),
        ];
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, content) in files {
            archive.start_file(name, options).unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_bounded_text_and_rejects_unknown_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "# Notes\nUse pnpm").unwrap();
        std::fs::write(dir.path().join("archive.bin"), [0, 1, 2]).unwrap();
        let document = extract_document(dir.path(), "notes.md").unwrap();
        assert!(document.content.contains("Use pnpm"));
        assert!(extract_document(dir.path(), "archive.bin").is_err());
    }

    #[test]
    fn extracts_document_data_urls_without_accepting_paths() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("# Notes\nUse pnpm");
        let document =
            extract_document_data_url("notes.md", &format!("data:text/markdown;base64,{encoded}"))
                .unwrap();

        assert_eq!(document.name, "notes.md");
        assert!(document.path.starts_with("attachment://"));
        assert!(document.content.contains("Use pnpm"));
        assert_eq!(document.source_bytes, 16);
        assert!(extract_document_data_url("../notes.md", "data:text/plain;base64,QQ==").is_err());
        assert!(extract_document_data_url("notes.md", "not-a-data-url").is_err());
        assert!(extract_document_data_url("notes.md", "data:text/plain\n;base64,QQ==").is_err());
        assert!(
            extract_document_data_url("archive.bin", "data:application/octet-stream;base64,AA==")
                .is_err()
        );
    }

    #[test]
    fn rejects_oversized_document_data_urls_before_decoding() {
        let encoded = "A".repeat((((MAX_DOCUMENT_BYTES as usize + 2) / 3) * 4) + 1);
        let error =
            extract_document_data_url("notes.txt", &format!("data:text/plain;base64,{encoded}"))
                .unwrap_err();

        assert!(error.contains("8 MiB"));
    }

    #[test]
    fn extracts_common_excel_attachments_in_sheet_order_as_tsv() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(test_xlsx());
        let cases = [
            (
                "budget.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            ("budget.xls", "application/vnd.ms-excel"),
            (
                "budget.xlsm",
                "application/vnd.ms-excel.sheet.macroEnabled.12",
            ),
            (
                "budget.xlsb",
                "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
            ),
        ];

        for (name, media_type) in cases {
            let document = extract_document_data_url(
                name,
                &format!("data:application/octet-stream;base64,{encoded}"),
            )
            .unwrap();
            assert_eq!(document.media_type, media_type);
            assert_eq!(
                document.content,
                "[工作表: 预算]\n项目\t金额\n住宿\t128.5\n\n[工作表: 备注]\n第一行\\n第二行\\t尾\n"
            );
            assert!(!document.truncated);
        }
    }

    #[test]
    fn enforces_spreadsheet_archive_expansion_limits() {
        let entry_count_error =
            validate_spreadsheet_archive_entry_count(MAX_SPREADSHEET_ARCHIVE_ENTRIES + 1)
                .unwrap_err();
        assert!(entry_count_error.contains("entries"));

        let entry_size_error =
            validate_spreadsheet_archive_sizes(&[MAX_SPREADSHEET_ARCHIVE_ENTRY_BYTES + 1])
                .unwrap_err();
        assert!(entry_size_error.contains("16 MiB"));

        let total_size_error =
            validate_spreadsheet_archive_sizes(&[MAX_SPREADSHEET_ARCHIVE_ENTRY_BYTES; 5])
                .unwrap_err();
        assert!(total_size_error.contains("64 MiB"));
    }
}
