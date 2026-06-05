// This file is part of the uutils grep package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

use memchr::memchr;
use std::fs::File;
use std::io::{self, Read as _};

pub struct LineBuffer {
    buffer: Vec<u8>,

    /// Start of the valid data and current (incomplete) line in `buffer`.
    beg: usize,
    /// Offset of where the next `memchr` should continue scanning from.
    /// This helps us avoid re-scanning bytes we've already confirmed don't contain a terminator.
    scan: usize,
    /// End of the current valid data in `buffer`.
    end: usize,

    /// Absolute file offset of `buffer[pos]`.
    next_line_start: u64,
    /// Set once a file read has returned EOF.
    eof: bool,
    /// The line terminator is typically NUL or LF.
    line_terminator: u8,
}

impl LineBuffer {
    pub fn new(line_terminator: u8) -> Self {
        Self {
            buffer: vec![0; 128 * 1024],

            beg: 0,
            scan: 0,
            end: 0,

            next_line_start: 0,
            eof: false,
            line_terminator,
        }
    }

    /// Reset the buffer to an empty state.
    pub fn reset(&mut self) {
        self.beg = 0;
        self.scan = 0;
        self.end = 0;
        self.next_line_start = 0;
        self.eof = false;
    }

    /// Read the next line from the given reader.
    /// Returns `Ok(None)` if the end of the reader is reached.
    /// Otherwise, returns `Ok(Some((line, line_start)))`, where `line` is the line read (without the
    /// `line_terminator`), and `line_start` is the absolute byte offset of the start of the line.
    pub fn read_line(&mut self, file: &mut File) -> io::Result<Option<(&[u8], u64)>> {
        if self.eof {
            return Ok(None);
        }

        loop {
            // Look for a line terminator and if found, yield that line.
            if let Some(off) = memchr(self.line_terminator, &self.buffer[self.scan..self.end]) {
                let line_start = self.next_line_start;
                let beg = self.beg;
                let end = self.scan + off;
                let line = &self.buffer[beg..end];
                self.beg = end + 1;
                self.scan = self.beg;
                self.next_line_start += (self.beg - beg) as u64;
                return Ok(Some((line, line_start)));
            }

            // `buffer[pos..end]` has no terminator. Remember that for the next scan.
            self.scan = self.end;

            // Move the partial line to the beginning of the buffer.
            // The idea is that read() calls will either be very slow, and this doesn't matter,
            // or they'll be very fast with big chunks and we want to maximize the amount of space per syscall.
            if self.beg > 0 {
                self.buffer.copy_within(self.beg..self.end, 0);
                self.end -= self.beg;
                self.scan -= self.beg;
                self.beg = 0;
            }
            if self.end == self.buffer.len() {
                // A single line exceeds the current buffer; grow.
                self.buffer.resize(self.buffer.len() * 2, 0);
            }

            // Read more data!
            let n = loop {
                match file.read(&mut self.buffer[self.end..]) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            };
            if n == 0 {
                // EOF: Yield the last line, if any.
                return if self.beg == self.end {
                    Ok(None)
                } else {
                    let line = &self.buffer[self.beg..self.end];
                    let line_start = self.next_line_start;
                    self.eof = true; // shortcut the next call to read_line()
                    Ok(Some((line, line_start)))
                };
            }
            self.end += n;
        }
    }
}
