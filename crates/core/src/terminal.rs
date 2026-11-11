//! Constrained VT display: UTF-8, CR/LF, cursor movement, erasing and basic SGR.
//! Wide glyphs, grapheme clusters, mouse reporting and full-screen apps are not claimed.
use std::collections::VecDeque;
pub struct Screen {
    pub columns: usize,
    pub rows: usize,
    cells: Vec<char>,
    x: usize,
    y: usize,
    escape: Vec<u8>,
    in_escape: bool,
    utf8: Vec<u8>,
    history: VecDeque<String>,
}
impl Screen {
    pub fn new(columns: usize, rows: usize) -> Self {
        let columns = columns.clamp(1, 240);
        let rows = rows.clamp(1, 100);
        Self {
            columns,
            rows,
            cells: vec![' '; columns * rows],
            x: 0,
            y: 0,
            escape: Vec::new(),
            in_escape: false,
            utf8: Vec::new(),
            history: VecDeque::new(),
        }
    }
    fn newline(&mut self) {
        if self.y + 1 < self.rows {
            self.y += 1;
        } else {
            let line = self.cells[..self.columns].iter().collect();
            self.history.push_back(line);
            if self.history.len() > 500 {
                self.history.pop_front();
            }
            self.cells.rotate_left(self.columns);
            let start = (self.rows - 1) * self.columns;
            self.cells[start..].fill(' ');
        }
    }
    fn print(&mut self, c: char) {
        if self.x >= self.columns {
            self.x = 0;
            self.newline();
        }
        self.cells[self.y * self.columns + self.x] = c;
        self.x += 1;
    }
    fn csi(&mut self, command: u8) {
        let bytes = &self.escape[1..self.escape.len() - 1];
        let text = String::from_utf8_lossy(bytes);
        let values: Vec<usize> = text
            .trim_start_matches('?')
            .split(';')
            .map(|x| x.parse().unwrap_or(0))
            .take(16)
            .collect();
        let first = values.first().copied().unwrap_or(0);
        let n = first.max(1);
        match command {
            b'A' => self.y = self.y.saturating_sub(n),
            b'B' => self.y = self.y.saturating_add(n).min(self.rows - 1),
            b'C' => self.x = self.x.saturating_add(n).min(self.columns - 1),
            b'D' => self.x = self.x.saturating_sub(n),
            b'G' => self.x = n.saturating_sub(1).min(self.columns - 1),
            b'H' | b'f' => {
                self.y = first.max(1).saturating_sub(1).min(self.rows - 1);
                self.x = values
                    .get(1)
                    .copied()
                    .unwrap_or(1)
                    .max(1)
                    .saturating_sub(1)
                    .min(self.columns - 1);
            }
            b'J' => {
                if first == 2 || first == 3 {
                    self.cells.fill(' ');
                } else {
                    let start = self.y * self.columns + self.x.min(self.columns - 1);
                    self.cells[start..].fill(' ');
                }
            }
            b'K' => {
                let start = self.y * self.columns;
                match first {
                    1 => self.cells[start..start + self.x.min(self.columns - 1) + 1].fill(' '),
                    2 => self.cells[start..start + self.columns].fill(' '),
                    _ => {
                        self.cells[start + self.x.min(self.columns)..start + self.columns].fill(' ')
                    }
                }
            }
            _ => {}
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.in_escape {
                if self.escape.len() >= 64 {
                    self.escape.clear();
                    self.in_escape = false;
                    continue;
                }
                self.escape.push(b);
                if self.escape[0] == b'[' {
                    if self.escape.len() > 1 && (0x40..=0x7e).contains(&b) {
                        self.csi(b);
                        self.in_escape = false;
                        self.escape.clear();
                    }
                } else if self.escape[0] == b']' {
                    if b == 7 || self.escape.ends_with(b"\x1b\\") {
                        self.in_escape = false;
                        self.escape.clear();
                    }
                } else {
                    self.in_escape = false;
                    self.escape.clear();
                }
                continue;
            }
            if !self.utf8.is_empty() {
                self.utf8.push(b);
                match std::str::from_utf8(&self.utf8) {
                    Ok(s) => {
                        let c = s.chars().next().unwrap();
                        self.utf8.clear();
                        self.print(c);
                    }
                    Err(e) if e.error_len().is_some() || self.utf8.len() > 4 => {
                        self.utf8.clear();
                        self.print('�');
                    }
                    _ => {}
                }
                continue;
            }
            match b {
                27 => {
                    self.in_escape = true;
                    self.escape.clear();
                }
                b'\r' => self.x = 0,
                b'\n' => self.newline(),
                8 => self.x = self.x.saturating_sub(1),
                b'\t' => self.x = ((self.x / 8 + 1) * 8).min(self.columns - 1),
                0..=31 | 127 => {}
                128..=255 => self.utf8.push(b),
                _ => self.print(b as char),
            }
        }
    }
    pub fn render(&self) -> String {
        let mut out = String::new();
        for row in &self.history {
            out.push_str(row.trim_end());
            out.push('\n');
        }
        for row in self.cells.chunks(self.columns) {
            let text: String = row.iter().collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hostile_cursor_parameters_are_bounded() {
        let mut screen = Screen::new(80, 24);
        screen.feed(format!("\x1b[{}B\x1b[{}CX", usize::MAX, usize::MAX).as_bytes());
        assert!(screen.render().contains('X'));
    }
    #[test]
    fn handles_chunked_utf8_cursor_and_erase() {
        let mut s = Screen::new(10, 3);
        s.feed(b"abc\rZ\x1b[K\n\r");
        let b = "🦀".as_bytes();
        s.feed(&b[..2]);
        s.feed(&b[2..]);
        assert!(s.render().starts_with("Z\n🦀\n"));
        s.feed(b"\x1b[2J\x1b[Hdone");
        assert!(s.render().starts_with("done"));
    }
}
