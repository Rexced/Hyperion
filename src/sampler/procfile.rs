use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// A kernel pseudo-file kept open and re-read from offset 0 each sample,
/// avoiding an open/close per tick. Works for procfs seq_files and sysfs attributes.
pub struct ProcFile {
    file: File,
    buf: String,
}

impl ProcFile {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            buf: String::with_capacity(4096),
        })
    }

    pub fn read(&mut self) -> io::Result<&str> {
        self.file.seek(SeekFrom::Start(0))?;
        self.buf.clear();
        self.file.read_to_string(&mut self.buf)?;
        Ok(&self.buf)
    }
}

/// Counter delta per second; a counter that went backwards (reset/wrap) yields 0.
pub fn rate(prev: u64, cur: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        return 0.0;
    }
    cur.saturating_sub(prev) as f64 / secs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rereads_changed_content() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "first").unwrap();
        let mut pf = ProcFile::open(tmp.path()).unwrap();
        assert_eq!(pf.read().unwrap(), "first");
        std::fs::write(tmp.path(), "second").unwrap();
        assert_eq!(pf.read().unwrap(), "second");
    }

    #[test]
    fn rate_handles_reset() {
        assert_eq!(rate(100, 300, 2.0), 100.0);
        assert_eq!(rate(300, 100, 1.0), 0.0);
        assert_eq!(rate(0, 100, 0.0), 0.0);
    }
}
