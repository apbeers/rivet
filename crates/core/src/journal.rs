//! Bounded, checksummed records. Torn tails are truncated; corruption is an error.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
const MAGIC: &[u8; 4] = b"RVJ1";
const LIMIT: usize = 1_048_576;
pub type Records = Vec<(u16, Vec<u8>)>;
pub struct Journal {
    file: File,
    sequence: u64,
}
fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
        }
    }
    !crc
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
impl Journal {
    pub fn open(path: &Path) -> io::Result<(Self, Records)> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        let mut records = Vec::new();
        let mut sequence = 0;
        let mut valid = 0;
        loop {
            let mut header = [0; 18];
            let mut n = 0;
            while n < header.len() {
                let got = file.read(&mut header[n..])?;
                if got == 0 {
                    break;
                }
                n += got;
            }
            if n == 0 {
                break;
            }
            if n < header.len() {
                file.set_len(valid)?;
                file.sync_all()?;
                break;
            }
            if &header[..4] != MAGIC {
                return Err(invalid("journal magic/version mismatch"));
            }
            let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            if len > LIMIT || valid.saturating_add(len as u64 + 22) > 64 * 1024 * 1024 {
                return Err(invalid("journal record exceeds budget"));
            }
            let seq = u64::from_le_bytes(header[8..16].try_into().unwrap());
            if seq != sequence + 1 {
                return Err(invalid("journal sequence mismatch"));
            }
            let kind = u16::from_le_bytes(header[16..18].try_into().unwrap());
            let mut tail = vec![0; len + 4];
            if let Err(e) = file.read_exact(&mut tail) {
                if e.kind() != io::ErrorKind::UnexpectedEof {
                    return Err(e);
                }
                file.set_len(valid)?;
                file.sync_all()?;
                break;
            }
            let expected = u32::from_le_bytes(tail[len..].try_into().unwrap());
            let mut checked = header.to_vec();
            checked.extend_from_slice(&tail[..len]);
            if checksum(&checked) != expected {
                return Err(invalid("journal checksum mismatch; original preserved"));
            }
            records.push((kind, tail[..len].to_vec()));
            sequence = seq;
            valid += 18 + len as u64 + 4;
            if records.len() > 100_000 {
                return Err(invalid("journal replay budget exceeded"));
            }
        }
        let file = OpenOptions::new().append(true).open(path)?;
        Ok((Self { file, sequence }, records))
    }
    /// Success means data reached durable storage. Caller must use a worker.
    pub fn append(&mut self, kind: u16, payload: &[u8]) -> io::Result<()> {
        if payload.len() > LIMIT {
            return Err(invalid("journal payload exceeds budget"));
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("sequence overflow"))?;
        let mut record = MAGIC.to_vec();
        record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        record.extend_from_slice(&sequence.to_le_bytes());
        record.extend_from_slice(&kind.to_le_bytes());
        record.extend_from_slice(payload);
        record.extend_from_slice(&checksum(&record).to_le_bytes());
        self.file.write_all(&record)?;
        self.file.sync_all()?;
        self.sequence = sequence;
        Ok(())
    }
}
pub fn user_data_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|p| PathBuf::from(p).join("Library/Application Support/Rivet"));
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Rivet"));
    #[cfg(not(any(target_os = "macos", windows)))]
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .map(|p| p.join("rivet"));
    base.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no per-user data directory"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovers_partial_tail_but_rejects_corruption() {
        let dir = std::env::temp_dir().join(format!("rivet-journal-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("log");
        let _ = fs::remove_file(&p);
        let (mut j, _) = Journal::open(&p).unwrap();
        j.append(1, "🦀".as_bytes()).unwrap();
        drop(j);
        OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"RVJ1\x12")
            .unwrap();
        let (mut j, records) = Journal::open(&p).unwrap();
        assert_eq!(records, vec![(1, "🦀".as_bytes().to_vec())]);
        j.append(2, b"next").unwrap();
        drop(j);
        assert_eq!(Journal::open(&p).unwrap().1.len(), 2);
        let mut data = fs::read(&p).unwrap();
        data[18] ^= 1;
        fs::write(&p, data).unwrap();
        assert!(Journal::open(&p).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
