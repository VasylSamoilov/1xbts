use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::auth;
use crate::message::{H_CONTACT, H_WWW_AUTHENTICATE, Headers, Method, SipRequest, SipResponse};

pub const SC_TRYING: u16 = 100;
pub const SC_OK: u16 = 200;
pub const SC_UNAUTHORIZED: u16 = 401;
pub const SC_FORBIDDEN: u16 = 403;
pub const SC_NOT_FOUND: u16 = 404;

const REASON_OK: &str = "OK";
const REASON_UNAUTHORIZED: &str = "Unauthorized";

const DEFAULT_EXPIRES: u32 = 3600;

fn dialog_headers(req: &SipRequest) -> Headers {
    let mut headers = Headers::new();
    // Every Via must be echoed in order for the response to route back.
    for via in req.headers.get_all(crate::message::H_VIA) {
        headers.append(crate::message::H_VIA, via);
    }
    if let Some(from) = req.headers.from() {
        headers.append(crate::message::H_FROM, from);
    }
    if let Some(to) = req.headers.to() {
        if req.headers.to_tag().is_some() {
            headers.append(crate::message::H_TO, to);
        } else {
            headers.append(crate::message::H_TO, format!("{};tag={}", to, new_tag(req)));
        }
    }
    if let Some(call_id) = req.headers.call_id() {
        headers.append(crate::message::H_CALL_ID, call_id);
    }
    if let Some(cseq) = req.headers.get(crate::message::H_CSEQ) {
        headers.append(crate::message::H_CSEQ, cseq);
    }
    headers
}

/// A deterministic To-tag so registration refreshes reuse the same tag.
fn new_tag(req: &SipRequest) -> String {
    let mut hasher = DefaultHasher::new();
    req.headers.call_id().unwrap_or_default().hash(&mut hasher);
    req.headers.to().unwrap_or_default().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn response(req: &SipRequest, status: u16, reason: &str) -> SipResponse {
    SipResponse {
        version: crate::message::SIP_VERSION.to_string(),
        status,
        reason: reason.to_string(),
        headers: dialog_headers(req),
        body: Vec::new(),
    }
}

pub fn challenge_response(req: &SipRequest, realm: &str, nonce: &str) -> SipResponse {
    let mut resp = response(req, SC_UNAUTHORIZED, REASON_UNAUTHORIZED);
    resp.headers
        .append(H_WWW_AUTHENTICATE, auth::challenge(realm, nonce, None));
    resp
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub contact: String,
    pub expires: u32,
}

#[derive(Debug, Default)]
pub struct Registrar {
    bindings: HashMap<String, Binding>,
}

impl Registrar {
    pub fn new() -> Self {
        Registrar::default()
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn binding(&self, aor: &str) -> Option<&Binding> {
        self.bindings.get(aor)
    }

    fn requested_expires(req: &SipRequest) -> u32 {
        if let Some(contact) = req.headers.contact() {
            if let Some(exp) = crate::message::param_value(contact, "expires") {
                if let Ok(v) = exp.parse() {
                    return v;
                }
            }
        }
        req.headers.expires().unwrap_or(DEFAULT_EXPIRES)
    }

    pub fn register(&mut self, req: &SipRequest) -> Option<Binding> {
        debug_assert_eq!(req.method, Method::Register);
        let aor = req.headers.to()?.to_string();
        let aor = aor_key(&aor);
        let contact = req.headers.contact().unwrap_or("").to_string();
        let expires = Self::requested_expires(req);

        if expires == 0 {
            self.bindings.remove(&aor);
            return None;
        }

        let binding = Binding { contact, expires };
        self.bindings.insert(aor, binding.clone());
        Some(binding)
    }

    pub fn ok_response(&self, req: &SipRequest) -> SipResponse {
        let mut resp = response(req, SC_OK, REASON_OK);
        let aor = req.headers.to().map(aor_key).unwrap_or_default();
        if let Some(binding) = self.bindings.get(&aor) {
            resp.headers.append(
                H_CONTACT,
                contact_with_expires(&binding.contact, binding.expires),
            );
        }
        resp
    }
}

fn aor_key(header: &str) -> String {
    let addr = match (header.find('<'), header.find('>')) {
        (Some(l), Some(r)) if r > l => &header[l + 1..r],
        _ => header.split(';').next().unwrap_or(header).trim(),
    };
    addr.trim().to_string()
}

fn contact_with_expires(contact: &str, expires: u32) -> String {
    let parameter_start = contact
        .find('>')
        .map(|index| index + 1)
        .or_else(|| contact.find(';'))
        .unwrap_or(contact.len());
    let (address, parameters) = contact.split_at(parameter_start);
    let retained = parameters
        .split(';')
        .map(str::trim)
        .filter(|parameter| {
            !parameter.is_empty()
                && !parameter
                    .split_once('=')
                    .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("expires"))
        })
        .collect::<Vec<_>>();
    if retained.is_empty() {
        format!("{address};expires={expires}")
    } else {
        format!("{address};{};expires={expires}", retained.join(";"))
    }
}
