//! UTF-8 piece table. Edits retain immutable spans rather than copying the file.
use std::{ops::Range, sync::Arc};
#[derive(Clone, Debug)]
struct Piece {
    source: Arc<str>,
    range: Range<usize>,
    units: usize,
}
impl Piece {
    fn span(&self, range: Range<usize>) -> Self {
        let units = if range == self.range {
            self.units
        } else {
            self.source[range.clone()].encode_utf16().count()
        };
        Self {
            source: self.source.clone(),
            range,
            units,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pieces: Vec<Piece>,
}
impl Snapshot {
    pub fn byte_len(&self) -> usize {
        self.pieces
            .iter()
            .fold(0usize, |n, p| n.saturating_add(p.range.len()))
    }
    pub fn text(&self) -> String {
        let mut text = String::new();
        for p in &self.pieces {
            text.push_str(&p.source[p.range.clone()]);
        }
        text
    }
    pub fn write_to(&self, out: &mut impl std::io::Write) -> std::io::Result<()> {
        for p in &self.pieces {
            out.write_all(p.source[p.range.clone()].as_bytes())?;
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct Buffer {
    pieces: Vec<Piece>,
    lines: Vec<(usize, usize)>,
    bytes: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}
impl Buffer {
    pub fn new(text: Arc<str>) -> Self {
        let bytes = text.len();
        let pieces = vec![Piece {
            range: 0..bytes,
            units: text.encode_utf16().count(),
            source: text,
        }];
        let mut b = Self {
            pieces,
            lines: Vec::new(),
            bytes,
            undo: Vec::new(),
            redo: Vec::new(),
        };
        b.reindex();
        b
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            pieces: self.pieces.clone(),
        }
    }
    pub fn len(&self) -> usize {
        self.bytes
    }
    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
    fn reindex(&mut self) {
        self.lines = vec![(0, 0)];
        let (mut bytes, mut utf16) = (0, 0);
        for p in &self.pieces {
            for c in p.source[p.range.clone()].chars() {
                bytes += c.len_utf8();
                utf16 += c.len_utf16();
                if c == '\n' {
                    self.lines.push((bytes, utf16));
                }
            }
        }
        self.bytes = bytes;
    }
    pub fn read(&self, range: Range<usize>) -> Result<String, String> {
        if range.start > range.end || range.end > self.bytes {
            return Err("range outside buffer".into());
        }
        let mut result = String::new();
        let mut offset = 0;
        for p in &self.pieces {
            let len = p.range.len();
            let start = range.start.saturating_sub(offset).min(len);
            let end = range.end.saturating_sub(offset).min(len);
            if start < end {
                let slice = p
                    .source
                    .get(p.range.start + start..p.range.start + end)
                    .ok_or("range splits UTF-8")?;
                result.push_str(slice);
            }
            offset += len;
        }
        Ok(result)
    }
    pub fn line_utf16(&self, line: usize) -> Option<usize> {
        self.lines.get(line).map(|(_, u)| *u)
    }
    pub fn position(&self, offset: usize) -> Result<(usize, usize), String> {
        self.utf16_to_byte(offset)?;
        let line = self
            .lines
            .partition_point(|(_, u)| *u <= offset)
            .saturating_sub(1);
        Ok((line, offset - self.lines[line].1))
    }
    pub fn utf16_to_byte(&self, index: usize) -> Result<usize, String> {
        let line = self
            .lines
            .partition_point(|(_, u)| *u <= index)
            .saturating_sub(1);
        let (mut pos, mut count) = self.lines[line];
        let mut offset = 0;
        for piece in &self.pieces {
            let end = offset + piece.range.len();
            if pos == offset && count + piece.units <= index {
                count += piece.units;
                pos = end;
            } else if end > pos {
                let skip = pos.saturating_sub(offset);
                for c in piece.source[piece.range.start + skip..piece.range.end].chars() {
                    if count == index {
                        return Ok(pos);
                    }
                    count += c.len_utf16();
                    pos += c.len_utf8();
                    if count > index {
                        return Err("UTF-16 position splits a surrogate pair".into());
                    }
                }
            }
            offset = end;
        }
        if count == index {
            Ok(pos)
        } else {
            Err("UTF-16 position outside buffer".into())
        }
    }
    pub fn byte_to_utf16(&self, index: usize) -> Result<usize, String> {
        if index > self.bytes {
            return Err("byte offset outside buffer".into());
        }
        let line = self
            .lines
            .partition_point(|(b, _)| *b <= index)
            .saturating_sub(1);
        let (start, mut units) = self.lines[line];
        let mut offset = 0;
        for piece in &self.pieces {
            let len = piece.range.len();
            let begin = start.saturating_sub(offset).min(len);
            let end = index.saturating_sub(offset).min(len);
            if begin == 0 && end == len {
                units += piece.units;
            } else if begin < end {
                units += piece
                    .source
                    .get(piece.range.start + begin..piece.range.start + end)
                    .ok_or("range splits UTF-8")?
                    .encode_utf16()
                    .count();
            }
            offset += len;
            if offset >= index {
                break;
            }
        }
        Ok(units)
    }
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Result<(), String> {
        if text.len() > 16 * 1024 * 1024
            || self
                .bytes
                .saturating_sub(range.len())
                .saturating_add(text.len())
                > 16 * 1024 * 1024
        {
            return Err("Document edit exceeds 16 MiB budget".into());
        }
        if range.start > range.end || range.end > self.bytes {
            return Err("invalid edit range".into());
        }
        let start_u = self.byte_to_utf16(range.start)?;
        let end_u = self.byte_to_utf16(range.end)?;
        let text_units = text.encode_utf16().count();
        if self.pieces.len() > 65_536 {
            return Err("piece budget reached; save and reopen".into());
        }
        self.undo.push(self.snapshot());
        if self.undo.len() > 128 {
            self.undo.remove(0);
        }
        self.redo.clear();
        let mut pieces = Vec::new();
        let mut offset = 0;
        let mut inserted = false;
        for p in &self.pieces {
            let end = offset + p.range.len();
            if offset < range.start {
                let keep = (range.start - offset).min(p.range.len());
                if keep > 0 {
                    pieces.push(p.span(p.range.start..p.range.start + keep));
                }
            }
            if !inserted && end >= range.start {
                if !text.is_empty() {
                    pieces.push(Piece {
                        source: Arc::from(text),
                        range: 0..text.len(),
                        units: text_units,
                    });
                }
                inserted = true;
            }
            if end > range.end {
                let skip = range.end.saturating_sub(offset).min(p.range.len());
                pieces.push(p.span(p.range.start + skip..p.range.end));
            }
            offset = end;
        }
        if !inserted && !text.is_empty() {
            pieces.push(Piece {
                source: Arc::from(text),
                range: 0..text.len(),
                units: text_units,
            });
        }
        // Maintain the line index by editing only affected line starts and shifting the tail.
        let removed_u = end_u - start_u;
        let (mut b, mut u) = (range.start, start_u);
        let mut added = Vec::new();
        for c in text.chars() {
            b += c.len_utf8();
            u += c.len_utf16();
            if c == '\n' {
                added.push((b, u));
            }
        }
        let inserted_u = u - start_u;
        let mut lines = Vec::with_capacity(self.lines.len() + added.len());
        for &(b, u) in &self.lines {
            if b <= range.start {
                lines.push((b, u));
            }
        }
        lines.extend(added);
        for &(b, u) in &self.lines {
            if b > range.end {
                lines.push((b - range.len() + text.len(), u - removed_u + inserted_u));
            }
        }
        self.bytes = self.bytes - range.len() + text.len();
        self.lines = lines;
        self.pieces = pieces;
        Ok(())
    }
    pub fn undo(&mut self) -> bool {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.pieces = previous.pieces;
            self.reindex();
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.pieces = next.pieces;
            self.reindex();
            true
        } else {
            false
        }
    }
    pub fn find(&self, needle: &str) -> Option<Range<usize>> {
        if needle.is_empty() {
            return None;
        }
        let bytes = needle.as_bytes();
        let mut prefix = vec![0; bytes.len()];
        for i in 1..bytes.len() {
            let mut j = prefix[i - 1];
            while j > 0 && bytes[i] != bytes[j] {
                j = prefix[j - 1];
            }
            if bytes[i] == bytes[j] {
                j += 1;
            }
            prefix[i] = j;
        }
        let (mut matched, mut offset) = (0, 0);
        for p in &self.pieces {
            for &b in p.source[p.range.clone()].as_bytes() {
                while matched > 0 && b != bytes[matched] {
                    matched = prefix[matched - 1];
                }
                if b == bytes[matched] {
                    matched += 1;
                }
                offset += 1;
                if matched == bytes.len() {
                    return Some(offset - bytes.len()..offset);
                }
            }
        }
        None
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Comment,
    String,
    Number,
    Keyword,
}
pub fn tokens(text: &str) -> Vec<(Range<usize>, TokenKind)> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() && tokens.len() < 512 {
        let start = i;
        let kind = if bytes[i] == b'#' || (bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/')) {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            Some(TokenKind::Comment)
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if b == b'\\' && i < bytes.len() {
                    i += 1;
                } else if b == quote {
                    break;
                }
            }
            Some(TokenKind::String)
        } else if bytes[i].is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.') {
                i += 1;
            }
            Some(TokenKind::Number)
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            matches!(
                &text[start..i],
                "fn" | "let"
                    | "mut"
                    | "pub"
                    | "struct"
                    | "impl"
                    | "use"
                    | "return"
                    | "if"
                    | "else"
                    | "for"
                    | "while"
                    | "class"
                    | "def"
                    | "import"
                    | "function"
                    | "const"
                    | "var"
                    | "async"
                    | "await"
            )
            .then_some(TokenKind::Keyword)
        } else {
            i += 1;
            None
        };
        if let Some(kind) = kind {
            tokens.push((start..i, kind));
        }
    }
    tokens
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_unicode_replacements_preserve_native_positions() {
        let mut reference = String::from("a🦀\n日本語\n");
        let mut buffer = Buffer::new(Arc::from(reference.as_str()));
        let mut seed = 123u64;
        for n in 0..300 {
            let mut random = || {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                seed as usize
            };
            let mut boundaries: Vec<_> = reference.char_indices().map(|(i, _)| i).collect();
            boundaries.push(reference.len());
            let a = boundaries[random() % boundaries.len()];
            let b = boundaries[random() % boundaries.len()];
            let range = a.min(b)..a.max(b);
            let text = ["🦀", "日本\n", "x", "", "\n"][random() % 5];
            buffer.replace(range.clone(), text).unwrap();
            reference.replace_range(range, text);
            assert_eq!(buffer.snapshot().text(), reference);
            let mut units = 0;
            for (byte, ch) in reference.char_indices() {
                assert_eq!(buffer.byte_to_utf16(byte).unwrap(), units);
                assert_eq!(buffer.utf16_to_byte(units).unwrap(), byte);
                if ch.len_utf16() == 2 {
                    assert!(buffer.utf16_to_byte(units + 1).is_err());
                }
                units += ch.len_utf16();
            }
            assert_eq!(buffer.utf16_to_byte(units).unwrap(), reference.len());
            if n % 20 == 0 {
                assert!(buffer.undo());
                assert!(buffer.redo());
                assert_eq!(buffer.snapshot().text(), reference);
            }
        }
    }
    #[test]
    fn unicode_edits_undo_redo_and_search_cross_pieces() {
        let mut b = Buffer::new(Arc::from("a🦀\nbeta\n"));
        assert_eq!(b.utf16_to_byte(3).unwrap(), 5);
        assert!(b.utf16_to_byte(2).is_err());
        b.replace(1..5, "日本\n語").unwrap();
        assert_eq!(b.snapshot().text(), "a日本\n語\nbeta\n");
        assert_eq!(b.line_count(), 4);
        assert_eq!(b.find("語\nbeta"), Some(8..16));
        assert!(b.undo());
        assert_eq!(b.snapshot().text(), "a🦀\nbeta\n");
        assert!(b.redo());
        assert_eq!(
            b.byte_to_utf16(b.len()).unwrap(),
            b.snapshot().text().encode_utf16().count()
        );
    }
    #[test]
    fn piece_edits_match_a_reference_string() {
        let mut b = Buffer::new(Arc::from("abc\ndef\n"));
        let mut reference = String::from("abc\ndef\n");
        for i in 0..500 {
            let at = i % (reference.len() + 1);
            b.replace(at..at, "x\n").unwrap();
            reference.insert_str(at, "x\n");
            assert_eq!(b.snapshot().text(), reference);
            assert_eq!(
                b.line_count(),
                reference.bytes().filter(|&c| c == b'\n').count() + 1
            );
        }
    }
}
