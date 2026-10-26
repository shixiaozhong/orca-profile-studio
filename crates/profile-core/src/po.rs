//! Read-only gettext catalogs. Profile loading and CLI diagnostics stay independent of locale.
use std::{collections::BTreeMap, fs, io, path::Path};

#[derive(Clone, Debug, Default)]
pub struct Translations {
    entries: BTreeMap<(Option<String>, String), String>,
    /// Lines that could not be read; their entries are left out, the rest of the catalog loads.
    skipped: usize,
}

#[derive(Default)]
struct Entry {
    context: Option<String>,
    id: Option<String>,
    text: String,
    fuzzy: bool,
    /// A `msgstr` was read, so a following comment starts the next entry.
    has_text: bool,
    broken: bool,
}
#[derive(Clone, Copy)]
enum Field {
    Context,
    Id,
    Text,
    Ignored,
}

impl Translations {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self::from_po(&fs::read_to_string(path)?))
    }

    /// Lines skipped because they were not valid gettext; each one drops only its own entry.
    pub fn skipped_lines(&self) -> usize {
        self.skipped
    }

    pub fn get(&self, message: &str) -> Option<&str> {
        self.get_context(None, message)
    }

    pub fn get_context(&self, context: Option<&str>, message: &str) -> Option<&str> {
        self.entries.get(&(context.map(str::to_owned), message.to_owned())).map(String::as_str)
    }

    /// Parses a catalog. Malformed lines are skipped (see [`Self::skipped_lines`]) instead of
    /// failing the whole file, so one odd line after an OrcaSlicer update loses one entry only.
    pub fn from_po(source: &str) -> Self {
        let mut catalog = Self::default();
        let mut entry = Entry::default();
        let mut field = Field::Ignored;
        let finish = |entry: &mut Entry, catalog: &mut Self| {
            let entry = std::mem::take(entry);
            if let Some(id) = entry.id
                && !id.is_empty()
                && !entry.text.is_empty()
                && !entry.fuzzy
                && !entry.broken
            {
                catalog.entries.insert((entry.context, id), entry.text);
            }
        };
        for line in source.trim_start_matches('\u{feff}').lines() {
            let line = line.trim();
            if line.is_empty() {
                finish(&mut entry, &mut catalog);
                field = Field::Ignored;
                continue;
            }
            if line.starts_with('#') {
                // Comments precede an entry; without a blank line in between they start a new one.
                if entry.has_text {
                    finish(&mut entry, &mut catalog);
                    field = Field::Ignored;
                }
                if let Some(flags) = line.strip_prefix("#,") {
                    entry.fuzzy |= flags.split(',').any(|s| s.trim() == "fuzzy");
                }
                continue;
            }
            let (next, literal) = if let Some(s) = line.strip_prefix("msgctxt ") {
                if entry.id.is_some() {
                    finish(&mut entry, &mut catalog);
                }
                entry.context = Some(String::new());
                (Field::Context, s)
            } else if let Some(s) = line.strip_prefix("msgid_plural ") {
                (Field::Ignored, s)
            } else if let Some(s) = line.strip_prefix("msgid ") {
                if entry.id.is_some() {
                    finish(&mut entry, &mut catalog);
                }
                entry.id = Some(String::new());
                (Field::Id, s)
            } else if let Some(s) = line.strip_prefix("msgstr ").or_else(|| line.strip_prefix("msgstr[0] ")) {
                (Field::Text, s)
            } else if let Some((_, s)) = line.strip_prefix("msgstr[").and_then(|s| s.split_once(']')) {
                (Field::Ignored, s.trim())
            } else if line.starts_with('"') {
                (field, line)
            } else {
                // An unknown keyword: drop this entry, keep reading.
                catalog.skipped += 1;
                entry.broken = true;
                field = Field::Ignored;
                continue;
            };
            let Ok(text) = decode(literal) else {
                catalog.skipped += 1;
                entry.broken = true;
                field = Field::Ignored;
                continue;
            };
            match next {
                Field::Context => entry.context.get_or_insert_default().push_str(&text),
                Field::Id => entry.id.get_or_insert_default().push_str(&text),
                Field::Text => {
                    entry.text.push_str(&text);
                    entry.has_text = true;
                }
                Field::Ignored => {}
            }
            field = next;
        }
        finish(&mut entry, &mut catalog);
        catalog
    }
}
fn decode(text: &str) -> Result<String, &'static str> {
    let text = text.strip_prefix('"').and_then(|s| s.strip_suffix('"')).ok_or("expected a quoted string")?;
    let mut result = Vec::new();
    let mut bytes = text.bytes().peekable();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            result.push(byte);
            continue;
        }
        let escaped = bytes.next().ok_or("incomplete escape")?;
        result.push(match escaped {
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'\\' | b'"' | b'\'' | b'?' => escaped,
            b'0'..=b'7' => {
                let mut value = (escaped - b'0') as u16;
                for _ in 0..2 {
                    if let Some(next @ b'0'..=b'7') = bytes.peek().copied() {
                        value = value * 8 + (next - b'0') as u16;
                        bytes.next();
                    } else {
                        break;
                    }
                }
                u8::try_from(value).map_err(|_| "octal escape out of range")?
            }
            b'x' => {
                let mut value = 0u16;
                let mut count = 0;
                while let Some(next) = bytes.peek().copied().and_then(|v| (v as char).to_digit(16)) {
                    value = value
                        .checked_mul(16)
                        .and_then(|v| v.checked_add(next as u16))
                        .ok_or("hex escape out of range")?;
                    bytes.next();
                    count += 1;
                }
                if count == 0 {
                    return Err("empty hex escape");
                }
                u8::try_from(value).map_err(|_| "hex escape out of range")?
            }
            _ => return Err("unknown escape"),
        });
    }
    String::from_utf8(result).map_err(|_| "invalid UTF-8 escape sequence")
}

#[cfg(test)]
mod tests {
    use super::Translations;
    #[test]
    fn parses_multiline_context_plural_and_skips_fuzzy_empty_and_obsolete() {
        let catalog = Translations::from_po(
            r#"
msgid ""
msgstr "Language: zh_CN\n"

msgid "Wall loops"
msgstr "墙层数"

msgctxt "menu"
msgid "Open"
msgstr "打开"

msgid "Line\n"
"two"
msgstr "第一行\n"
"第二行"

msgid "item"
msgid_plural "items"
msgstr[0] "项目"
msgstr[1] "ignored"

#, fuzzy, c-format
msgid "old"
msgstr "旧"

msgid "empty"
msgstr ""

#~ msgid "obsolete"
#~ msgstr "旧条目"
"#,
        );
        assert_eq!(catalog.get("Wall loops"), Some("墙层数"));
        assert_eq!(catalog.get("Open"), None);
        assert_eq!(catalog.get_context(Some("menu"), "Open"), Some("打开"));
        assert_eq!(catalog.get("Line\ntwo"), Some("第一行\n第二行"));
        assert_eq!(catalog.get("item"), Some("项目"));
        for key in ["", "old", "empty", "obsolete"] {
            assert_eq!(catalog.get(key), None);
        }
    }
    #[test]
    fn decodes_c_escapes_and_rejects_malformed_catalogs() {
        let catalog = Translations::from_po("\u{feff}msgid \"a\"\r\nmsgstr \"\\\"\\\\\\t\\101\\x42\"\r\n");
        assert_eq!(catalog.get("a"), Some("\"\\\tAB"));
        assert_eq!(catalog.skipped_lines(), 0);
        // A bad line drops its own entry only; the rest of the catalog still loads.
        let catalog = Translations::from_po(
            "msgid \"bad\"\nmsgstr \"\\q\"\n\nmsgid nope\nmsgstr \"x\"\n\nfuture_keyword \"?\"\nmsgid \"dropped\"\nmsgstr \"丢弃\"\n\nmsgid \"kept\"\nmsgstr \"保留\"\n",
        );
        assert_eq!(catalog.get("bad"), None);
        assert_eq!(catalog.get("dropped"), None, "an unknown keyword drops the entry it belongs to");
        assert_eq!(catalog.get("kept"), Some("保留"));
        assert_eq!(catalog.skipped_lines(), 3);
        // A fuzzy flag directly after an entry belongs to the next entry, not the previous one.
        let catalog = Translations::from_po("msgid \"a\"\nmsgstr \"甲\"\n#, fuzzy\nmsgid \"b\"\nmsgstr \"乙\"\n");
        assert_eq!(catalog.get("a"), Some("甲"));
        assert_eq!(catalog.get("b"), None);
    }
}
