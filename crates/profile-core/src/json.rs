use std::hash::{DefaultHasher, Hash, Hasher};
use std::{fs, io, path::Path};

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

pub(crate) struct JsonFile {
    pub value: serde_json::Value,
    pub had_bom: bool,
}

/// Identifies file contents within one process, to detect files that changed after loading.
pub(crate) fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum JsonError {
    #[error("cannot read file: {0}")]
    Io(#[from] io::Error),
    #[error("invalid JSON: {0}")]
    Parse(#[from] serde_json::Error),
}

pub(crate) fn read_json(path: &Path) -> Result<JsonFile, JsonError> {
    parse_json(&fs::read(path)?)
}

pub(crate) fn parse_json(bytes: &[u8]) -> Result<JsonFile, JsonError> {
    // nlohmann::json (used by OrcaSlicer) skips a UTF-8 BOM, serde_json rejects it.
    let (body, had_bom) = match bytes.strip_prefix(UTF8_BOM) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    Ok(JsonFile { value: serde_json::from_slice(body)?, had_bom })
}
