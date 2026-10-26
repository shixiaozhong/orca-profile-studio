//! A preset file kept as text, so edits rewrite only the values that change.
//!
//! Profiles are hand-maintained and reviewed through Git, so saving must not reformat a file:
//! indentation (4 spaces, 2 spaces or tabs), inline vs. multi-line arrays, a missing final
//! newline and a UTF-8 BOM all stay as they are. Only top-level members are tracked; that is all
//! a preset file has.

use std::ops::Range;

use crate::model::RawValue;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a JSON object at byte {offset}: {reason}")]
pub struct DocumentError {
    pub offset: usize,
    pub reason: &'static str,
}

#[derive(Debug, Clone)]
struct Member {
    key: String,
    /// From the opening quote of the key.
    start: usize,
    value: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct Document {
    text: String,
    /// Byte offsets of the outer `{` and `}`.
    open: usize,
    close: usize,
    members: Vec<Member>,
}

impl Document {
    pub fn parse(text: impl Into<String>) -> Result<Self, DocumentError> {
        let text = text.into();
        let mut scanner = Scanner { bytes: text.as_bytes(), pos: 0 };
        if scanner.bytes.starts_with(b"\xEF\xBB\xBF") {
            scanner.pos = 3;
        }
        scanner.skip_ws();
        let open = scanner.pos;
        scanner.expect(b'{', "expected `{`")?;
        let mut members = Vec::new();
        scanner.skip_ws();
        if scanner.peek() != Some(b'}') {
            loop {
                scanner.skip_ws();
                let start = scanner.pos;
                let key_span = scanner.string()?;
                let key = decode_string(&text[key_span]).ok_or(scanner.error("invalid key"))?;
                scanner.skip_ws();
                scanner.expect(b':', "expected `:`")?;
                scanner.skip_ws();
                let value = scanner.value()?;
                members.push(Member { key, start, value });
                scanner.skip_ws();
                match scanner.peek() {
                    Some(b',') => scanner.pos += 1,
                    Some(b'}') => break,
                    _ => return Err(scanner.error("expected `,` or `}`")),
                }
            }
        }
        let close = scanner.pos;
        scanner.expect(b'}', "expected `}`")?;
        scanner.skip_ws();
        if scanner.pos != scanner.bytes.len() {
            return Err(scanner.error("trailing characters"));
        }
        Ok(Document { text, open, close, members })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }

    pub fn contains(&self, key: &str) -> bool {
        self.members.iter().any(|member| member.key == key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|member| member.key.as_str())
    }

    /// A top-level JSON value, including manifest arrays of objects.
    pub fn value(&self, key: &str) -> Option<serde_json::Value> {
        let member = self.members.iter().find(|member| member.key == key)?;
        serde_json::from_str(&self.text[member.value.clone()]).ok()
    }

    fn array_spans(&self, key: &str) -> Result<(usize, usize, Vec<Range<usize>>), DocumentError> {
        let member = self
            .members
            .iter()
            .find(|member| member.key == key)
            .ok_or(DocumentError { offset: self.close, reason: "array member not found" })?;
        let mut scanner = Scanner { bytes: self.text.as_bytes(), pos: member.value.start };
        let open = scanner.pos;
        scanner.expect(b'[', "expected an array")?;
        scanner.skip_ws();
        let mut spans = Vec::new();
        while scanner.peek() != Some(b']') {
            spans.push(scanner.value()?);
            scanner.skip_ws();
            if scanner.peek() == Some(b',') {
                scanner.pos += 1;
                scanner.skip_ws();
            } else {
                break;
            }
        }
        scanner.expect(b']', "expected `]`")?;
        Ok((open, scanner.pos - 1, spans))
    }

    /// Inserts one array element while retaining every existing element byte-for-byte.
    pub fn array_insert(&mut self, key: &str, ix: usize, value: &serde_json::Value) -> Result<(), DocumentError> {
        if !self.contains(key) && ix != 0 {
            return Err(DocumentError { offset: self.close, reason: "array index out of bounds" });
        }
        if !self.contains(key) {
            self.set(key, &RawValue::List(Vec::new()));
        }
        let (open, close, spans) = self.array_spans(key)?;
        if ix > spans.len() {
            return Err(DocumentError { offset: close, reason: "array index out of bounds" });
        }
        let multiline = self.text[open..close].contains('\n');
        let indent = spans
            .first()
            .map(|span| self.member_indent(span.start).to_owned())
            .unwrap_or_else(|| format!("{}{}", self.member_indent(open), self.indent()));
        let separator = if multiline {
            format!(",{}{indent}", self.newline())
        } else if spans.len() > 1 {
            self.text[spans[0].end..spans[1].start].to_owned()
        } else {
            ", ".to_owned()
        };
        let rendered = serde_json::to_string(value).expect("JSON values serialize");
        if let Some(next) = spans.get(ix) {
            self.replace(next.start..next.start, &format!("{rendered}{separator}"));
        } else if let Some(last) = spans.last() {
            self.replace(last.end..last.end, &format!("{separator}{rendered}"));
        } else if multiline {
            self.replace(open + 1..open + 1, &format!("{}{indent}{rendered}", self.newline()));
        } else {
            self.replace(close..close, &rendered);
        }
        Ok(())
    }

    /// Replaces just one element, leaving siblings, separators and surrounding whitespace intact.
    pub fn array_set(&mut self, key: &str, ix: usize, value: &serde_json::Value) -> Result<(), DocumentError> {
        let (_, close, spans) = self.array_spans(key)?;
        let span = spans.get(ix).ok_or(DocumentError { offset: close, reason: "array index out of bounds" })?.clone();
        if serde_json::from_str::<serde_json::Value>(&self.text[span.clone()]).ok().as_ref() != Some(value) {
            self.replace(span, &serde_json::to_string(value).expect("JSON values serialize"));
        }
        Ok(())
    }

    pub fn array_remove(&mut self, key: &str, ix: usize) -> Result<(), DocumentError> {
        let (_, close, spans) = self.array_spans(key)?;
        let span = spans.get(ix).ok_or(DocumentError { offset: close, reason: "array index out of bounds" })?;
        let range = if let Some(next) = spans.get(ix + 1) {
            span.start..next.start
        } else if ix > 0 {
            spans[ix - 1].end..span.end
        } else {
            span.clone()
        };
        self.replace(range, "");
        Ok(())
    }

    /// Edits one member inside an array's object without reformatting that object or its siblings.
    pub fn array_set_member(
        &mut self,
        key: &str,
        ix: usize,
        member: &str,
        value: &RawValue,
    ) -> Result<(), DocumentError> {
        let (_, close, spans) = self.array_spans(key)?;
        let span = spans.get(ix).ok_or(DocumentError { offset: close, reason: "array index out of bounds" })?.clone();
        let mut object = Self::parse(self.text[span.clone()].to_owned())?;
        object.set(member, value);
        self.replace(span, object.text());
        Ok(())
    }

    /// Sets a top-level member, keeping the style of the value it replaces. A new member is
    /// appended after the last one, using the file's indentation. Setting the value a member
    /// already has leaves the text untouched.
    pub fn set(&mut self, key: &str, value: &RawValue) {
        let positions: Vec<usize> = (0..self.members.len()).filter(|&ix| self.members[ix].key == key).collect();
        if positions.is_empty() {
            self.insert(key, value);
            return;
        }
        // Unique keys can retain unchanged array elements verbatim, including unusual whitespace
        // and escaped strings. Duplicate keys keep the existing all-occurrences replacement rule.
        if positions.len() == 1
            && let RawValue::List(after) = value
            && let Some(serde_json::Value::Array(before)) = self.value(key)
        {
            if after.is_empty() && !before.is_empty() {
                // Removing items one by one would leave the multi-line layout's blank line behind.
                let span = self.members[positions[0]].value.clone();
                self.replace(span, "[]");
                return;
            }
            let after: Vec<_> = after.iter().map(|item| serde_json::Value::String(item.clone())).collect();
            self.update_array(key, &before, &after);
            return;
        }
        // Back to front, so earlier spans stay valid.
        for &ix in positions.iter().rev() {
            let span = self.members[ix].value.clone();
            if serde_json::from_str::<RawValue>(&self.text[span.clone()]).ok().as_ref() == Some(value) {
                continue;
            }
            let rendered = self.render_replacement(&self.text[span.clone()], span.start, value);
            self.replace(span, &rendered);
        }
    }

    fn update_array(&mut self, key: &str, before: &[serde_json::Value], after: &[serde_json::Value]) {
        // Retain LCS matches verbatim. Profiles normally have short lists;
        // for very large arrays use common prefix/suffix to bound both memory and work.
        let mut pairs = Vec::new();
        if before.len().saturating_mul(after.len()) <= 1_000_000 {
            let width = after.len() + 1;
            let mut lengths = vec![0usize; (before.len() + 1) * width];
            for i in (0..before.len()).rev() {
                for j in (0..after.len()).rev() {
                    lengths[i * width + j] = if before[i] == after[j] {
                        lengths[(i + 1) * width + j + 1] + 1
                    } else {
                        lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
                    };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < before.len() && j < after.len() {
                if before[i] == after[j] {
                    pairs.push((i, j));
                    i += 1;
                    j += 1;
                } else if lengths[(i + 1) * width + j] >= lengths[i * width + j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        } else {
            let prefix = before.iter().zip(after).take_while(|(a, b)| a == b).count();
            pairs.extend((0..prefix).map(|i| (i, i)));
            let suffix =
                before[prefix..].iter().rev().zip(after[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
            pairs.extend((0..suffix).rev().map(|i| (before.len() - 1 - i, after.len() - 1 - i)));
        }
        // Work backwards through unmatched gaps. Replace paired elements in place rather than
        // removing the last element and rebuilding an empty multiline array.
        pairs.push((before.len(), after.len()));
        for gap in (0..pairs.len()).rev() {
            let (old_start, new_start) = if gap == 0 { (0, 0) } else { (pairs[gap - 1].0 + 1, pairs[gap - 1].1 + 1) };
            let (old_end, new_end) = pairs[gap];
            let common = (old_end - old_start).min(new_end - new_start);
            for offset in 0..common {
                self.array_set(key, old_start + offset, &after[new_start + offset]).expect("parsed array index");
            }
            for ix in (old_start + common..old_end).rev() {
                self.array_remove(key, ix).expect("parsed array index");
            }
            for (offset, value) in after[new_start + common..new_end].iter().enumerate() {
                self.array_insert(key, old_start + common + offset, value).expect("parsed array index");
            }
        }
    }

    /// Removes every top-level member named `key` together with its separator.
    pub fn remove(&mut self, key: &str) {
        while let Some(ix) = self.members.iter().position(|member| member.key == key) {
            let range = if ix > 0 {
                // `,<newline><indent>"key": value` after the previous member.
                self.members[ix - 1].value.end..self.members[ix].value.end
            } else if let Some(next) = self.members.get(1) {
                // The first member: `"key": value,<newline><indent>` up to the next key.
                self.members[0].start..next.start
            } else {
                self.open + 1..self.close
            };
            self.replace(range, "");
        }
    }

    fn insert(&mut self, key: &str, value: &RawValue) {
        let indent = self.indent();
        let newline = self.newline();
        let member_text = format!("{}: {}", quote(key), render(value, &Style::multi_line(indent, newline)));
        match self.members.last() {
            Some(last) => {
                let at = last.value.end;
                self.replace(at..at, &format!(",{newline}{indent}{member_text}"));
            }
            None => {
                let range = self.open + 1..self.close;
                self.replace(range, &format!("{newline}{indent}{member_text}{newline}"));
            }
        }
    }

    fn replace(&mut self, range: Range<usize>, with: &str) {
        let mut text = std::mem::take(&mut self.text);
        text.replace_range(range, with);
        *self = Document::parse(text).expect("an edit of a valid document stays valid");
    }

    /// Indentation of the members, taken from the first one; four spaces for an empty object.
    fn indent(&self) -> &str {
        let Some(first) = self.members.first() else { return "    " };
        let line_start = self.text[..first.start].rfind('\n').map_or(0, |at| at + 1);
        let prefix = &self.text[line_start..first.start];
        if prefix.chars().all(|c| c == ' ' || c == '\t') { prefix } else { "    " }
    }

    fn newline(&self) -> &'static str {
        if self.text.contains("\r\n") { "\r\n" } else { "\n" }
    }

    fn render_replacement(&self, old: &str, old_start: usize, value: &RawValue) -> String {
        let newline = self.newline();
        let style = match (old.as_bytes().first(), value) {
            (Some(b'['), RawValue::List(_)) if !old.contains('\n') => {
                Style::Inline { separator: if old.contains("\",\"") { "," } else { ", " } }
            }
            (Some(b'['), RawValue::List(_)) => {
                // Keep the item and closing indentation of the array being replaced.
                let leading = |line: &'_ str| line.len() - line.trim_start().len();
                let item = old.split('\n').nth(1).map(|line| &line[..leading(line)]).unwrap_or_default();
                let last = old.rsplit('\n').next().unwrap_or_default();
                let close = &last[..leading(last)];
                if item.is_empty() {
                    Style::multi_line(self.member_indent(old_start), newline)
                } else {
                    Style::MultiLine { item: item.to_owned(), close: close.to_owned(), newline }
                }
            }
            _ => Style::multi_line(self.member_indent(old_start), newline),
        };
        render(value, &style)
    }

    /// Indentation of the line a value starts on.
    fn member_indent(&self, value_start: usize) -> &str {
        let line_start = self.text[..value_start].rfind('\n').map_or(0, |at| at + 1);
        let line = &self.text[line_start..value_start];
        &line[..line.len() - line.trim_start().len()]
    }
}

enum Style {
    Inline { separator: &'static str },
    MultiLine { item: String, close: String, newline: &'static str },
}

impl Style {
    /// nlohmann::json's `setw` layout, which most profiles use: items one level deeper.
    fn multi_line(indent: &str, newline: &'static str) -> Style {
        Style::MultiLine { item: indent.repeat(2), close: indent.to_owned(), newline }
    }
}

fn render(value: &RawValue, style: &Style) -> String {
    match value {
        RawValue::Str(text) => quote(text),
        RawValue::List(items) if items.is_empty() => "[]".to_owned(),
        RawValue::List(items) => match style {
            Style::Inline { separator } => {
                format!("[{}]", items.iter().map(|item| quote(item)).collect::<Vec<_>>().join(separator))
            }
            Style::MultiLine { item, close, newline } => {
                let lines: Vec<String> = items.iter().map(|text| format!("{item}{}", quote(text))).collect();
                format!("[{newline}{}{newline}{close}]", lines.join(&format!(",{newline}")))
            }
        },
    }
}

/// JSON string escaping; serde_json escapes exactly like nlohmann::json's `dump()`.
fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("strings always serialize")
}

fn decode_string(literal: &str) -> Option<String> {
    serde_json::from_str(literal).ok()
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn error(&self, reason: &'static str) -> DocumentError {
        DocumentError { offset: self.pos, reason }
    }

    fn expect(&mut self, byte: u8, reason: &'static str) -> Result<(), DocumentError> {
        if self.peek() != Some(byte) {
            return Err(self.error(reason));
        }
        self.pos += 1;
        Ok(())
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// A string literal including its quotes.
    fn string(&mut self) -> Result<Range<usize>, DocumentError> {
        let start = self.pos;
        self.expect(b'"', "expected a string")?;
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(start..self.pos);
                }
                Some(b'\\') => self.pos += 2,
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Any JSON value; only its extent matters.
    fn value(&mut self) -> Result<Range<usize>, DocumentError> {
        let start = self.pos;
        match self.peek() {
            Some(b'"') => {
                self.string()?;
            }
            Some(open @ (b'[' | b'{')) => {
                let close = if open == b'[' { b']' } else { b'}' };
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(close) {
                    self.pos += 1;
                    return Ok(start..self.pos);
                }
                loop {
                    self.skip_ws();
                    if open == b'{' {
                        self.string()?;
                        self.skip_ws();
                        self.expect(b':', "expected `:`")?;
                        self.skip_ws();
                    }
                    self.value()?;
                    self.skip_ws();
                    match self.peek() {
                        Some(b',') => self.pos += 1,
                        Some(byte) if byte == close => {
                            self.pos += 1;
                            break;
                        }
                        _ => return Err(self.error("expected `,` or a closing bracket")),
                    }
                }
            }
            Some(_) => {
                while self.peek().is_some_and(|byte| !matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'))
                {
                    self.pos += 1;
                }
                if self.pos == start {
                    return Err(self.error("expected a value"));
                }
            }
            None => return Err(self.error("unexpected end")),
        }
        Ok(start..self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::Document;
    use crate::model::RawValue;

    const FILE: &str = "{\n    \"type\": \"filament\",\n    \"name\": \"Acme PLA\",\n    \"nozzle_temperature\": [\n        \"210\",\n        \"215\"\n    ],\n    \"compatible_printers\": [\"A\", \"B\"]\n}";

    #[test]
    fn array_edits_keep_siblings_bom_crlf_and_nested_object_style() {
        let source = "\u{feff}{\r\n\t\"list\": [\r\n\t\t{ \"name\" : \"A\", \"sub_path\": \"a.json\" },\r\n\t\t{\"name\":\"B\",\"sub_path\":\"b.json\"}\r\n\t]\r\n}";
        let mut doc = Document::parse(source).unwrap();
        doc.array_set_member("list", 0, "name", &RawValue::Str("New A".into())).unwrap();
        assert_eq!(doc.text(), source.replace("\"A\"", "\"New A\""));
        doc.array_insert("list", 1, &serde_json::json!({"name":"C"})).unwrap();
        assert!(doc.text().contains("{ \"name\" : \"New A\", \"sub_path\": \"a.json\" },\r\n\t\t{\"name\":\"C\"},\r\n\t\t{\"name\":\"B\",\"sub_path\":\"b.json\"}"));
        doc.array_remove("list", 1).unwrap();
        assert_eq!(doc.text(), source.replace("\"A\"", "\"New A\""));
        doc.array_remove("list", 1).unwrap();
        doc.array_remove("list", 0).unwrap();
        doc.array_insert("list", 0, &serde_json::json!("new")).unwrap();
        assert_eq!(doc.value("list").unwrap(), serde_json::json!(["new"]));
    }

    #[test]
    fn inline_arrays_and_new_metadata_keep_untouched_members_valid() {
        let mut doc = Document::parse(r#"{"x":[ "a" ,  "b" ],"other":"keep"}"#).unwrap();
        doc.array_insert("x", 1, &serde_json::json!("new")).unwrap();
        assert_eq!(doc.text(), r#"{"x":[ "a" ,  "new" ,  "b" ],"other":"keep"}"#);
        doc.array_remove("x", 1).unwrap();
        doc.set("name", &RawValue::Str("new name".into()));
        assert_eq!(doc.value("other").unwrap(), "keep");
        serde_json::from_str::<serde_json::Value>(doc.text()).unwrap();
        assert!(doc.array_set("x", 4, &serde_json::json!("bad")).is_err());
    }

    fn list(items: &[&str]) -> RawValue {
        RawValue::List(items.iter().map(|item| item.to_string()).collect())
    }

    fn edited(edit: impl FnOnce(&mut Document)) -> String {
        let mut doc = Document::parse(FILE).unwrap();
        edit(&mut doc);
        let text = doc.into_text();
        serde_json::from_str::<serde_json::Value>(&text).expect("still valid JSON");
        text
    }

    #[test]
    fn array_diff_preserves_escaped_elements_and_irregular_separators() {
        let source = "\u{feff}{\r\n\t\"items\": [\r\n\t\t\"\\u0041\", \r\n\t\t\"B\",\r\n  \"C\"\r\n\t],\r\n\t\"untouched\": \"yes\"\r\n}";
        let mut doc = Document::parse(source).unwrap();
        doc.set("items", &list(&["A", "B", "D", "C"]));
        assert_eq!(doc.text(), source.replace("\"C\"", "\"D\",\r\n\t\t\"C\""));
        doc.set("items", &list(&["A", "C"]));
        assert!(doc.text().contains("\"\\u0041\""));
        assert!(doc.text().contains("\"untouched\": \"yes\""));
        assert_eq!(doc.value("items").unwrap(), serde_json::json!(["A", "C"]));
    }

    #[test]
    fn array_diff_handles_empty_duplicate_and_reordered_elements() {
        let cases = [
            vec![],
            vec!["a"],
            vec!["b"],
            vec!["a", "b"],
            vec!["b", "a"],
            vec!["a", "a", "b"],
            vec!["b", "a", "b", "c"],
        ];
        for before in &cases {
            for after in &cases {
                for multiline in [false, true] {
                    let separator = if multiline { ",\r\n  " } else { ",  " };
                    let items = before.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(separator);
                    let source = if multiline {
                        format!("{{\r\n \"items\": [\r\n  {items}\r\n ],\r\n \"other\":\"x\"\r\n}}")
                    } else {
                        format!("{{\"items\":[{items}],\"other\":\"x\"}}")
                    };
                    let mut doc = Document::parse(source.clone()).unwrap();
                    doc.set("items", &list(after));
                    assert_eq!(doc.value("items").unwrap(), serde_json::json!(after), "{before:?} -> {after:?}");
                    assert_eq!(doc.value("other").unwrap(), "x");
                    if before == after {
                        assert_eq!(doc.text(), source);
                    }
                    if after.is_empty() && !before.is_empty() {
                        assert!(doc.text().contains("\"items\":[]") || doc.text().contains("\"items\": []"));
                    }
                }
            }
        }
    }

    #[test]
    fn replaces_values_in_their_own_style() {
        let text = edited(|doc| doc.set("nozzle_temperature", &list(&["220"])));
        assert_eq!(text, FILE.replace("[\n        \"210\",\n        \"215\"\n    ]", "[\n        \"220\"\n    ]"));
        let text = edited(|doc| doc.set("compatible_printers", &list(&["C"])));
        assert_eq!(text, FILE.replace("[\"A\", \"B\"]", "[\"C\"]"));
        let text = edited(|doc| doc.set("name", &RawValue::Str("Acme \"PLA\"\n2".into())));
        assert!(text.contains("\"name\": \"Acme \\\"PLA\\\"\\n2\","));
    }

    #[test]
    fn inserts_and_removes_members() {
        let text = edited(|doc| doc.set("filament_id", &RawValue::Str("GFL99".into())));
        assert!(text.ends_with("[\"A\", \"B\"],\n    \"filament_id\": \"GFL99\"\n}"), "{text}");
        let text = edited(|doc| doc.set("bed_temperature", &list(&["60", "65"])));
        assert!(text.ends_with(",\n    \"bed_temperature\": [\n        \"60\",\n        \"65\"\n    ]\n}"), "{text}");

        let text = edited(|doc| doc.remove("nozzle_temperature"));
        assert_eq!(
            text,
            "{\n    \"type\": \"filament\",\n    \"name\": \"Acme PLA\",\n    \"compatible_printers\": [\"A\", \"B\"]\n}"
        );
        let text = edited(|doc| doc.remove("type"));
        assert!(text.starts_with("{\n    \"name\": \"Acme PLA\",\n"), "{text}");
        let text = edited(|doc| doc.remove("compatible_printers"));
        assert!(text.ends_with("        \"215\"\n    ]\n}"), "{text}");
    }

    #[test]
    fn keeps_bom_tabs_and_missing_final_newline() {
        let source = "\u{feff}{\n\t\"a\": \"1\"\n}";
        let mut doc = Document::parse(source).unwrap();
        doc.set("b", &list(&["x"]));
        assert_eq!(doc.text(), "\u{feff}{\n\t\"a\": \"1\",\n\t\"b\": [\n\t\t\"x\"\n\t]\n}");
        doc.remove("a");
        doc.remove("b");
        assert_eq!(doc.text(), "\u{feff}{}");
        doc.set("c", &RawValue::Str("3".into()));
        assert_eq!(doc.text(), "\u{feff}{\n    \"c\": \"3\"\n}");
    }

    /// Rendering every value of the real profiles in the style of the value it replaces gives
    /// back the original text, except for four arrays with stray whitespace.
    #[test]
    fn reproduces_the_formatting_of_every_profile_value() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::var_os("ORCA_PROFILES_DIR")
            .map(|dir| workspace.join(dir))
            .unwrap_or_else(|| workspace.join("../OrcaSlicer/resources/profiles"));
        if !dir.is_dir() {
            return;
        }
        let mut files = Vec::new();
        collect_json(&dir, &mut files);
        let mut checked = 0;
        let mut mismatches = Vec::new();
        for path in &files {
            let doc = Document::parse(std::fs::read_to_string(path).unwrap()).unwrap();
            for member in &doc.members {
                let old = &doc.text[member.value.clone()];
                let Ok(value) = serde_json::from_str::<RawValue>(old) else { continue };
                checked += 1;
                let rendered = doc.render_replacement(old, member.value.start, &value);
                if rendered != old {
                    let visible = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                    assert!(
                        visible(&rendered) == visible(old),
                        "{}: `{}` differs in more than whitespace",
                        path.display(),
                        member.key
                    );
                    mismatches.push(format!("{}: {}", path.strip_prefix(&dir).unwrap().display(), member.key));
                }
            }
        }
        assert!(checked > 190_000, "checked {checked} values");
        mismatches.sort();
        assert_eq!(
            mismatches,
            [
                "Afinia/filament/Afinia PLA.json: filament_vendor",
                "FlyingBear/machine/Ghost7/FlyingBear Ghost7 0.4 nozzle.json: default_filament_profile",
                "OrcaFilamentLibrary.json: filament_list",
                "Tiertime/filament/Tiertime PA6-CF@300HS.json: compatible_printers",
            ]
        );
    }

    fn collect_json(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for path in std::fs::read_dir(dir).unwrap().filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_dir() {
                collect_json(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "json") {
                out.push(path);
            }
        }
    }

    #[test]
    fn setting_the_current_value_changes_nothing() {
        let mut doc = Document::parse("{\n  \"a\": [ \"1\" ]\n}").unwrap();
        doc.set("a", &list(&["1"]));
        assert_eq!(doc.text(), "{\n  \"a\": [ \"1\" ]\n}");
    }

    #[test]
    fn rejects_non_objects() {
        assert!(Document::parse("[1, 2]").is_err());
        assert!(Document::parse("{\"a\": \"1\"} x").is_err());
        assert!(Document::parse("{\"a\": \"1\",}").is_err());
    }
}
