//! The small slice of HTTP/1.1 the audio.cpp server needs: a GET, and a chunked live-PCM exchange over loopback.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub struct Head {
    pub status: u16,
    pub chunked: bool,
    pub content_length: Option<usize>,
}

pub fn connect(addr: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

pub fn read_head<R: BufRead>(reader: &mut R) -> io::Result<Head> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("bad status line: {line:?}")))?;
    let (mut chunked, mut content_length) = (false, None);
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            match name.to_ascii_lowercase().as_str() {
                "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
                "content-length" => content_length = value.parse().ok(),
                _ => {}
            }
        }
    }
    Ok(Head { status, chunked, content_length })
}

/// Reads one chunk of a chunked body; `None` at the terminating zero-size chunk.
pub fn read_chunk<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("bad chunk size: {line:?}")))?;
    if size == 0 {
        // trailers (usually none) up to the blank line
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || line.trim_end().is_empty() {
                return Ok(None);
            }
        }
    }
    let mut data = vec![0u8; size];
    reader.read_exact(&mut data)?;
    let mut crlf = [0u8; 2];
    reader.read_exact(&mut crlf)?;
    Ok(Some(data))
}

pub fn write_chunk<W: Write>(writer: &mut W, data: &[u8]) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let mut buf = Vec::with_capacity(data.len() + 16);
    buf.extend_from_slice(format!("{:x}\r\n", data.len()).as_bytes());
    buf.extend_from_slice(data);
    buf.extend_from_slice(b"\r\n");
    writer.write_all(&buf)
}

/// `GET path`, returning the status and the body (small responses only).
pub fn get(addr: SocketAddr, path: &str, timeout: Duration) -> io::Result<(u16, String)> {
    let mut stream = connect(addr, timeout)?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")?;
    let mut reader = BufReader::new(stream);
    let head = read_head(&mut reader)?;
    let mut body = Vec::new();
    if head.chunked {
        while let Some(chunk) = read_chunk(&mut reader)? {
            body.extend_from_slice(&chunk);
        }
    } else {
        reader.take(head.content_length.unwrap_or(1 << 20) as u64).read_to_end(&mut body)?;
    }
    Ok((head.status, String::from_utf8_lossy(&body).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_round_trip_and_terminator() {
        let mut wire = Vec::new();
        write_chunk(&mut wire, b"hello").unwrap();
        write_chunk(&mut wire, &[7u8; 20]).unwrap();
        wire.extend_from_slice(b"0\r\n\r\n");
        let mut r = BufReader::new(&wire[..]);
        assert_eq!(read_chunk(&mut r).unwrap().unwrap(), b"hello");
        assert_eq!(read_chunk(&mut r).unwrap().unwrap(), vec![7u8; 20]);
        assert!(read_chunk(&mut r).unwrap().is_none());
    }

    #[test]
    fn head_parsing() {
        let wire = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nX: y\r\n\r\n";
        let head = read_head(&mut BufReader::new(&wire[..])).unwrap();
        assert!(head.status == 200 && head.chunked && head.content_length.is_none());
        assert!(read_head(&mut BufReader::new(&b"garbage\r\n\r\n"[..])).is_err());
    }
}
