//! SIP message model and wire codec (RFC 3261).

use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum SipError {
    #[error("message is empty")]
    Empty,
    #[error("malformed start line: {0}")]
    StartLine(String),
    #[error("malformed header line: {0}")]
    HeaderLine(String),
    #[error("invalid status code: {0}")]
    StatusCode(String),
    #[error("declared Content-Length {declared} exceeds available body {available}")]
    ContentLength { declared: usize, available: usize },
    #[error("message is not valid UTF-8 up to the body")]
    Utf8,
}

pub const SIP_VERSION: &str = "SIP/2.0";
const CRLF: &str = "\r\n";

const M_REGISTER: &str = "REGISTER";
const M_INVITE: &str = "INVITE";
const M_ACK: &str = "ACK";
const M_CANCEL: &str = "CANCEL";
const M_BYE: &str = "BYE";
const M_OPTIONS: &str = "OPTIONS";
const M_MESSAGE: &str = "MESSAGE";
const M_PRACK: &str = "PRACK";
const M_UPDATE: &str = "UPDATE";
const M_INFO: &str = "INFO";

pub const H_VIA: &str = "Via";
pub const H_FROM: &str = "From";
pub const H_TO: &str = "To";
pub const H_CALL_ID: &str = "Call-ID";
pub const H_CSEQ: &str = "CSeq";
pub const H_CONTACT: &str = "Contact";
pub const H_EXPIRES: &str = "Expires";
pub const H_MAX_FORWARDS: &str = "Max-Forwards";
pub const H_CONTENT_TYPE: &str = "Content-Type";
pub const H_CONTENT_LENGTH: &str = "Content-Length";
pub const H_WWW_AUTHENTICATE: &str = "WWW-Authenticate";
pub const H_AUTHORIZATION: &str = "Authorization";
pub const H_ROUTE: &str = "Route";
pub const H_RECORD_ROUTE: &str = "Record-Route";
pub const H_SUPPORTED: &str = "Supported";
pub const H_REQUIRE: &str = "Require";
pub const H_ALLOW: &str = "Allow";
pub const H_USER_AGENT: &str = "User-Agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Register,
    Invite,
    Ack,
    Cancel,
    Bye,
    Options,
    Message,
    Prack,
    Update,
    Info,
    Other(String),
}

impl Method {
    pub fn as_str(&self) -> &str {
        match self {
            Method::Register => M_REGISTER,
            Method::Invite => M_INVITE,
            Method::Ack => M_ACK,
            Method::Cancel => M_CANCEL,
            Method::Bye => M_BYE,
            Method::Options => M_OPTIONS,
            Method::Message => M_MESSAGE,
            Method::Prack => M_PRACK,
            Method::Update => M_UPDATE,
            Method::Info => M_INFO,
            Method::Other(s) => s,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for Method {
    fn from(s: &str) -> Self {
        match s {
            M_REGISTER => Method::Register,
            M_INVITE => Method::Invite,
            M_ACK => Method::Ack,
            M_CANCEL => Method::Cancel,
            M_BYE => Method::Bye,
            M_OPTIONS => Method::Options,
            M_MESSAGE => Method::Message,
            M_PRACK => Method::Prack,
            M_UPDATE => Method::Update,
            M_INFO => Method::Info,
            other => Method::Other(other.to_string()),
        }
    }
}

/// Order and original spelling of each field are preserved for re-serialization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    fields: Vec<(String, String)>,
}

impl Headers {
    pub fn new() -> Self {
        Headers::default()
    }

    pub fn append(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.fields.push((name.into(), value.into()));
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.remove(name);
        self.fields.push((name.to_string(), value.into()));
    }

    pub fn remove(&mut self, name: &str) {
        self.fields.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.fields
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    pub fn via(&self) -> Option<&str> {
        self.get(H_VIA)
    }

    pub fn via_branch(&self) -> Option<String> {
        param_value(self.via()?, "branch")
    }

    pub fn from(&self) -> Option<&str> {
        self.get(H_FROM)
    }

    pub fn from_tag(&self) -> Option<String> {
        param_value(self.from()?, "tag")
    }

    pub fn to(&self) -> Option<&str> {
        self.get(H_TO)
    }

    pub fn to_tag(&self) -> Option<String> {
        param_value(self.to()?, "tag")
    }

    pub fn call_id(&self) -> Option<&str> {
        self.get(H_CALL_ID)
    }

    pub fn cseq(&self) -> Option<(u32, String)> {
        let raw = self.get(H_CSEQ)?;
        let mut parts = raw.split_whitespace();
        let num = parts.next()?.parse().ok()?;
        let method = parts.next()?.to_string();
        Some((num, method))
    }

    pub fn contact(&self) -> Option<&str> {
        self.get(H_CONTACT)
    }

    pub fn expires(&self) -> Option<u32> {
        self.get(H_EXPIRES)?.trim().parse().ok()
    }

    pub fn max_forwards(&self) -> Option<u32> {
        self.get(H_MAX_FORWARDS)?.trim().parse().ok()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.get(H_CONTENT_TYPE)
    }

    pub fn content_length(&self) -> Option<usize> {
        self.get(H_CONTENT_LENGTH)?.trim().parse().ok()
    }

    pub fn www_authenticate(&self) -> Option<&str> {
        self.get(H_WWW_AUTHENTICATE)
    }

    pub fn authorization(&self) -> Option<&str> {
        self.get(H_AUTHORIZATION)
    }

    pub fn route(&self) -> Vec<&str> {
        self.get_all(H_ROUTE)
    }

    pub fn record_route(&self) -> Vec<&str> {
        self.get_all(H_RECORD_ROUTE)
    }

    pub fn supported(&self) -> Option<&str> {
        self.get(H_SUPPORTED)
    }

    pub fn require(&self) -> Option<&str> {
        self.get(H_REQUIRE)
    }

    pub fn allow(&self) -> Option<&str> {
        self.get(H_ALLOW)
    }

    pub fn user_agent(&self) -> Option<&str> {
        self.get(H_USER_AGENT)
    }
}

pub fn param_value(header: &str, name: &str) -> Option<String> {
    for part in header.split(';').skip(1) {
        let part = part.trim();
        let (k, v) = part.split_once('=')?;
        if k.trim().eq_ignore_ascii_case(name) {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SipRequest {
    pub method: Method,
    pub uri: String,
    pub version: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl SipRequest {
    pub fn new(method: Method, uri: impl Into<String>) -> Self {
        SipRequest {
            method,
            uri: uri.into(),
            version: SIP_VERSION.to_string(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SipResponse {
    pub version: String,
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl SipResponse {
    pub fn new(status: u16, reason: impl Into<String>) -> Self {
        SipResponse {
            version: SIP_VERSION.to_string(),
            status,
            reason: reason.into(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SipMessage {
    Request(SipRequest),
    Response(SipResponse),
}

impl SipMessage {
    pub fn headers(&self) -> &Headers {
        match self {
            SipMessage::Request(r) => &r.headers,
            SipMessage::Response(r) => &r.headers,
        }
    }

    pub fn body(&self) -> &[u8] {
        match self {
            SipMessage::Request(r) => &r.body,
            SipMessage::Response(r) => &r.body,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<SipMessage, SipError> {
        parse(bytes)
    }

    pub fn serialize(&self) -> Vec<u8> {
        serialize(self)
    }
}

pub fn parse(bytes: &[u8]) -> Result<SipMessage, SipError> {
    if bytes.is_empty() {
        return Err(SipError::Empty);
    }

    // The header block is US-ASCII, but the body may be arbitrary bytes, so
    // split on the CRLFCRLF (or LFLF) boundary before decoding text.
    let (head, body) = split_head_body(bytes);
    let head = std::str::from_utf8(head).map_err(|_| SipError::Utf8)?;

    let unfolded = unfold(head);
    let mut lines = unfolded.split(CRLF).filter(|l| !l.is_empty());
    let start = lines.next().ok_or(SipError::Empty)?;

    let mut headers = Headers::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| SipError::HeaderLine(line.to_string()))?;
        headers.append(name.trim(), value.trim());
    }

    // Trust Content-Length when present, but never read past what we have.
    let body = match headers.content_length() {
        Some(len) if len <= body.len() => body[..len].to_vec(),
        Some(len) => {
            return Err(SipError::ContentLength {
                declared: len,
                available: body.len(),
            });
        }
        None => body.to_vec(),
    };

    if start.starts_with(SIP_VERSION) {
        parse_response(start, headers, body).map(SipMessage::Response)
    } else {
        parse_request(start, headers, body).map(SipMessage::Request)
    }
}

fn parse_request(start: &str, headers: Headers, body: Vec<u8>) -> Result<SipRequest, SipError> {
    let mut parts = start.splitn(3, ' ');
    let method = parts
        .next()
        .ok_or_else(|| SipError::StartLine(start.to_string()))?;
    let uri = parts
        .next()
        .ok_or_else(|| SipError::StartLine(start.to_string()))?;
    let version = parts
        .next()
        .ok_or_else(|| SipError::StartLine(start.to_string()))?;
    Ok(SipRequest {
        method: Method::from(method),
        uri: uri.to_string(),
        version: version.to_string(),
        headers,
        body,
    })
}

fn parse_response(start: &str, headers: Headers, body: Vec<u8>) -> Result<SipResponse, SipError> {
    let mut parts = start.splitn(3, ' ');
    let version = parts
        .next()
        .ok_or_else(|| SipError::StartLine(start.to_string()))?;
    let code = parts
        .next()
        .ok_or_else(|| SipError::StartLine(start.to_string()))?;
    let reason = parts.next().unwrap_or("");
    let status = code
        .parse::<u16>()
        .map_err(|_| SipError::StatusCode(code.to_string()))?;
    Ok(SipResponse {
        version: version.to_string(),
        status,
        reason: reason.to_string(),
        headers,
        body,
    })
}

fn split_head_body(bytes: &[u8]) -> (&[u8], &[u8]) {
    if let Some(pos) = find(bytes, b"\r\n\r\n") {
        (&bytes[..pos + 2], &bytes[pos + 4..])
    } else if let Some(pos) = find(bytes, b"\n\n") {
        (&bytes[..pos + 1], &bytes[pos + 2..])
    } else {
        (bytes, &[])
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A following line that begins with SP or HTAB is a wrapped (folded) header value.
fn unfold(head: &str) -> String {
    let normalized = head.replace("\r\n", "\n");
    let mut out = String::with_capacity(normalized.len());
    for line in normalized.split('\n') {
        if line.starts_with(' ') || line.starts_with('\t') {
            out.push(' ');
            out.push_str(line.trim_start());
        } else {
            if !out.is_empty() {
                out.push_str(CRLF);
            }
            out.push_str(line);
        }
    }
    out.push_str(CRLF);
    out
}

pub fn serialize(msg: &SipMessage) -> Vec<u8> {
    match msg {
        SipMessage::Request(r) => serialize_request(r),
        SipMessage::Response(r) => serialize_response(r),
    }
}

fn serialize_request(req: &SipRequest) -> Vec<u8> {
    let start = format!("{} {} {}", req.method, req.uri, req.version);
    serialize_common(&start, &req.headers, &req.body)
}

fn serialize_response(resp: &SipResponse) -> Vec<u8> {
    let start = format!("{} {} {}", resp.version, resp.status, resp.reason);
    serialize_common(&start, &resp.headers, &resp.body)
}

fn serialize_common(start: &str, headers: &Headers, body: &[u8]) -> Vec<u8> {
    let mut text = String::new();
    text.push_str(start);
    text.push_str(CRLF);
    for (name, value) in headers.iter() {
        // Content-Length is authoritative from the body, emitted below.
        if name.eq_ignore_ascii_case(H_CONTENT_LENGTH) {
            continue;
        }
        text.push_str(name);
        text.push_str(": ");
        text.push_str(value);
        text.push_str(CRLF);
    }
    text.push_str(H_CONTENT_LENGTH);
    text.push_str(": ");
    text.push_str(&body.len().to_string());
    text.push_str(CRLF);
    text.push_str(CRLF);

    let mut out = text.into_bytes();
    out.extend_from_slice(body);
    out
}
