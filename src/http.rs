//! The subset of HTTP/1.1 this needs, and no more.
//!
//! Writing a server by hand is normally the wrong call. It is the right one
//! here for the reason `Cargo.toml` gives: the node compiles its own checkouts
//! on a LAN that never reaches a registry. The subset is small because the
//! surface is small -- two routes, one content type, no keep-alive, no TLS, no
//! uploads, no ranges, no cookies -- and every one of those absences is a
//! feature that cannot then be got wrong.
//!
//! What a hand-written server must not get wrong, and what is therefore
//! deliberate below: a read timeout, so a connection that opens and says
//! nothing cannot hold a thread forever; a cap on the request line, on the
//! header block and on the body, so none of them can be made to grow until the
//! process dies; and headers that are written on every response rather than
//! remembered per route.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const MAX_LINE: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_BODY: usize = 64 * 1024;
pub const READ_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Request {
    pub method: String,
    /// Path with any query string already cut off and trailing slash trimmed.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Why a request could not be read, and what to answer.
///
/// A request this server refuses still deserves a reply. Dropping the
/// connection makes a client report a network fault for what was really a
/// rule -- and a person debugging a fleet at nine in the morning should be
/// told "that request is too large", not left with a closed socket.
pub struct ParseError {
    pub code: u16,
    pub msg: String,
    /// True when nothing readable arrived at all, in which case there is
    /// nobody to answer and the connection is simply dropped.
    pub silent: bool,
}

fn bad(code: u16, msg: &str) -> ParseError {
    ParseError { code, msg: msg.to_string(), silent: false }
}

fn hung_up() -> ParseError {
    ParseError { code: 400, msg: "the connection said nothing".into(), silent: true }
}

fn io_err(e: io::Error) -> ParseError {
    ParseError { code: 400, msg: format!("could not read the request: {}", e), silent: true }
}

pub fn read_request(stream: &TcpStream) -> Result<Request, ParseError> {
    stream.set_read_timeout(Some(READ_TIMEOUT)).map_err(io_err)?;
    let mut reader = BufReader::new(stream);

    let mut line = String::new();
    let n = (&mut reader)
        .take(MAX_LINE as u64)
        .read_line(&mut line)
        .map_err(io_err)?;
    if n == 0 {
        return Err(hung_up());
    }
    if n >= MAX_LINE {
        return Err(bad(413, "request line too long"));
    }
    let mut parts = line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| bad(400, "no method"))?
        .to_string();
    let target = parts.next().ok_or_else(|| bad(400, "no target"))?;

    // Only the path is ever used, and it is trimmed the same way for every
    // route so that /api/verb/ and /api/verb cannot be two different things.
    let path = target.split('?').next().unwrap_or("/");
    let path = path.trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };

    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        let n = (&mut reader)
            .take(MAX_LINE as u64)
            .read_line(&mut h)
            .map_err(io_err)?;
        if n == 0 {
            return Err(bad(400, "headers ended early"));
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADERS {
            return Err(bad(431, "too many headers"));
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(bad(413, "that request is too large"));
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body).map_err(io_err)?;
    }

    Ok(Request {
        method,
        path: path.to_string(),
        headers,
        body,
    })
}

pub struct Response {
    pub code: u16,
    pub ctype: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(code: u16, body: impl Into<Vec<u8>>) -> Self {
        Response { code, ctype: "application/json; charset=utf-8", body: body.into() }
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Response { code: 200, ctype: "text/html; charset=utf-8", body: body.into() }
    }
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

pub fn write_response(stream: &mut TcpStream, res: &Response, head_only: bool) -> io::Result<()> {
    // Invariant 6 is a LAN, not an absence of care. The page loads nothing
    // from anywhere, so the policy that says so costs nothing to enforce; and
    // a fleet console inside someone else's frame is a clickjacking target
    // with real verbs behind it, so framing is denied outright.
    let head = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         X-Frame-Options: DENY\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self'; img-src 'self' data:; \
         style-src 'unsafe-inline'; script-src 'unsafe-inline'; frame-ancestors 'none'\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        res.code,
        reason(res.code),
        res.ctype,
        res.body.len()
    );
    stream.write_all(head.as_bytes())?;
    if !head_only {
        stream.write_all(&res.body)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_trimmed_the_same_way_every_time() {
        // The route table is compared against exact strings, so these must all
        // land on one spelling or a verb route grows a second, unguarded door.
        for (target, want) in [
            ("/api/verb", "/api/verb"),
            ("/api/verb/", "/api/verb"),
            ("/api/verb?x=1", "/api/verb"),
            ("/api/verb/?x=1", "/api/verb"),
            ("/", "/"),
            ("/?a=b", "/"),
        ] {
            let path = target.split('?').next().unwrap_or("/");
            let path = path.trim_end_matches('/');
            let path = if path.is_empty() { "/" } else { path };
            assert_eq!(path, want, "target {:?}", target);
        }
    }

    #[test]
    fn headers_are_found_regardless_of_case() {
        let r = Request {
            method: "POST".into(),
            path: "/api/verb".into(),
            headers: vec![("X-Copal-Token".into(), "abc".into())],
            body: vec![],
        };
        assert_eq!(r.header("x-copal-token"), Some("abc"));
        assert_eq!(r.header("X-COPAL-TOKEN"), Some("abc"));
        assert_eq!(r.header("nope"), None);
    }
}
