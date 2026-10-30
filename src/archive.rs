use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};

mod rar;
pub(crate) use rar::{extract_rar, is_rar, open_rar, rar_link, rar_name, rar_options};

const CAPACITY: usize = 256 * 1024;

pub(crate) struct ArchiveReader<R> {
    inner: BufReader<R>,
    position: u64,
}

impl<R: Read + Seek> ArchiveReader<R> {
    pub(crate) fn new(mut inner: R) -> io::Result<Self> {
        let position = inner.stream_position()?;
        Ok(Self {
            inner: BufReader::with_capacity(CAPACITY, inner),
            position,
        })
    }
}

impl<R: Read + Seek> Read for ArchiveReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(bytes)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for ArchiveReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(target) => target,
            SeekFrom::Current(offset) => self
                .position
                .checked_add_signed(offset)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ZIP 读取位置无效"))?,
            SeekFrom::End(offset) => {
                self.position = self.inner.seek(SeekFrom::End(offset))?;
                return Ok(self.position);
            }
        };
        if let Ok(offset) = i64::try_from(i128::from(target) - i128::from(self.position)) {
            self.inner.seek_relative(offset)?;
        } else {
            self.inner.seek(SeekFrom::Start(target))?;
        }
        self.position = target;
        Ok(target)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.position)
    }
}

pub(crate) struct ArchiveWriter<W> {
    inner: W,
    buffer: Vec<u8>,
    start: u64,
    cursor: usize,
}

impl<W: Write + Seek> ArchiveWriter<W> {
    pub(crate) fn new(mut inner: W) -> io::Result<Self> {
        let start = inner.stream_position()?;
        Ok(Self {
            inner,
            buffer: Vec::with_capacity(CAPACITY),
            start,
            cursor: 0,
        })
    }

    fn commit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let position = self.start + self.cursor as u64;
        self.inner.seek(SeekFrom::Start(self.start))?;
        self.inner.write_all(&self.buffer)?;
        if self.cursor != self.buffer.len() {
            self.inner.seek(SeekFrom::Start(position))?;
        }
        self.start = position;
        self.cursor = 0;
        self.buffer.clear();
        Ok(())
    }
}

impl<W: Write + Seek> Write for ArchiveWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.cursor == CAPACITY {
            self.commit()?;
        }
        let count = bytes.len().min(CAPACITY - self.cursor);
        let end = self.cursor + count;
        if end > self.buffer.len() {
            self.buffer.resize(end, 0);
        }
        self.buffer[self.cursor..end].copy_from_slice(&bytes[..count]);
        self.cursor = end;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.commit()?;
        self.inner.flush()
    }
}

impl<W: Write + Seek> Seek for ArchiveWriter<W> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => position,
            SeekFrom::Current(offset) => (self.start + self.cursor as u64)
                .checked_add_signed(offset)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ZIP 写入位置无效"))?,
            SeekFrom::End(offset) => {
                self.commit()?;
                self.start = self.inner.seek(SeekFrom::End(offset))?;
                return Ok(self.start);
            }
        };
        if position >= self.start && position - self.start <= self.buffer.len() as u64 {
            self.cursor = (position - self.start) as usize;
        } else {
            self.commit()?;
            self.start = self.inner.seek(SeekFrom::Start(position))?;
        }
        Ok(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn buffered_reader_matches_cursor_across_seek_boundaries() {
        let bytes: Vec<_> = (0..CAPACITY * 3).map(|index| (index % 251) as u8).collect();
        let mut reference = Cursor::new(bytes.clone());
        let mut reader = ArchiveReader::new(Cursor::new(bytes)).unwrap();
        for index in 0..600 {
            let target = if index % 3 == 0 {
                SeekFrom::End(-((index * 31) as i64 + 64))
            } else if index % 3 == 1 {
                SeekFrom::Start((index * 7907 % (CAPACITY * 3 - 64)) as u64)
            } else {
                SeekFrom::Current(-24)
            };
            assert_eq!(
                reader.seek(target).unwrap(),
                reference.seek(target).unwrap()
            );
            let mut a = [0; 32];
            let mut b = [0; 32];
            reader.read_exact(&mut a).unwrap();
            reference.read_exact(&mut b).unwrap();
            assert_eq!(a, b);
            assert_eq!(reader.stream_position().unwrap(), reference.position());
        }
        reader.rewind().unwrap();
        assert!(reader.seek(SeekFrom::Current(-1)).is_err());
        assert_eq!(reader.stream_position().unwrap(), 0);
    }
}
