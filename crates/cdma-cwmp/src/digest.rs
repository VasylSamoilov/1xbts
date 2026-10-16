//! HTTP Digest access authentication (RFC 2617, MD5) for the TR-069 boundary.

use md5::{Digest, Md5};

const ALGORITHM_MD5: &str = "MD5";
pub const QOP_AUTH: &str = "auth";

/// When `qop` is `Some`, RFC 2617 requires `nc` and `cnonce` and folds them into the response hash. When it is `None` the legacy unprotected form is emitted.
#[derive(Debug, Clone)]
pub struct DigestParams<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub realm: &'a str,
    pub nonce: &'a str,
    pub method: &'a str,
    pub uri: &'a str,
    pub qop: Option<&'a str>,
    pub cnonce: Option<&'a str>,
    pub nc: Option<&'a str>,
    pub opaque: Option<&'a str>,
}

fn md5_hex(data: &str) -> String {
    let mut h = Md5::new();
    h.update(data.as_bytes());
    hex::encode(h.finalize())
}

fn ha1(username: &str, realm: &str, password: &str) -> String {
    md5_hex(&format!("{username}:{realm}:{password}"))
}

fn ha2(method: &str, uri: &str) -> String {
    md5_hex(&format!("{method}:{uri}"))
}

/// Compute the `response` value per RFC 2617 §3.2.2.1.
fn response(
    ha1: &str,
    nonce: &str,
    ha2: &str,
    qop: Option<&str>,
    nc: Option<&str>,
    cnonce: Option<&str>,
) -> Option<String> {
    match qop {
        Some(qop) => {
            let nc = nc?;
            let cnonce = cnonce?;
            Some(md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}")))
        }
        None => Some(md5_hex(&format!("{ha1}:{nonce}:{ha2}"))),
    }
}

pub fn digest_authorization(p: &DigestParams) -> String {
    let ha1 = ha1(p.username, p.realm, p.password);
    let ha2 = ha2(p.method, p.uri);
    let resp = response(&ha1, p.nonce, &ha2, p.qop, p.nc, p.cnonce)
        .unwrap_or_else(|| md5_hex(&format!("{ha1}:{}:{ha2}", p.nonce)));

    let mut parts = vec![
        format!("username=\"{}\"", p.username),
        format!("realm=\"{}\"", p.realm),
        format!("nonce=\"{}\"", p.nonce),
        format!("uri=\"{}\"", p.uri),
        format!("algorithm={ALGORITHM_MD5}"),
        format!("response=\"{resp}\""),
    ];
    if let Some(qop) = p.qop {
        parts.push(format!("qop={qop}"));
        if let Some(nc) = p.nc {
            parts.push(format!("nc={nc}"));
        }
        if let Some(cnonce) = p.cnonce {
            parts.push(format!("cnonce=\"{cnonce}\""));
        }
    }
    if let Some(opaque) = p.opaque {
        parts.push(format!("opaque=\"{opaque}\""));
    }
    format!("Digest {}", parts.join(", "))
}

fn parse_digest_params(header: &str) -> Vec<(String, String)> {
    let body = header
        .trim()
        .strip_prefix("Digest")
        .unwrap_or(header)
        .trim();
    let mut out = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let (value, next) = if let Some(after) = after.strip_prefix('"') {
            match after.find('"') {
                Some(end) => (after[..end].to_string(), &after[end + 1..]),
                None => (after.to_string(), ""),
            }
        } else {
            match after.find(',') {
                Some(end) => (after[..end].trim().to_string(), &after[end..]),
                None => (after.trim().to_string(), ""),
            }
        };
        out.push((key, value));
        rest = next
            .trim_start()
            .strip_prefix(',')
            .unwrap_or(next)
            .trim_start();
    }
    out
}

fn find<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

#[derive(Debug, Clone, Default)]
pub struct DigestChallenge {
    pub realm: Option<String>,
    pub nonce: Option<String>,
    pub qop: Option<String>,
    pub opaque: Option<String>,
    pub algorithm: Option<String>,
}

pub fn parse_challenge(header: &str) -> DigestChallenge {
    let params = parse_digest_params(header);
    let get = |key: &str| find(&params, key).map(str::to_string);
    let qop = get("qop")
        .map(|value| {
            if value.split(',').any(|token| token.trim() == QOP_AUTH) {
                QOP_AUTH.to_string()
            } else {
                value
                    .split(',')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            }
        })
        .filter(|value| !value.is_empty());
    DigestChallenge {
        realm: get("realm"),
        nonce: get("nonce"),
        qop,
        opaque: get("opaque"),
        algorithm: get("algorithm"),
    }
}

/// The verifier is the authority on `nonce` and `realm`, so a mismatch there is rejected before hashing.
pub fn verify_digest(
    auth_header: &str,
    expected_username: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    method: &str,
) -> bool {
    let params = parse_digest_params(auth_header);
    let Some(username) = find(&params, "username") else {
        return false;
    };
    if username != expected_username {
        return false;
    }
    if find(&params, "realm") != Some(realm) {
        return false;
    }
    if find(&params, "nonce") != Some(nonce) {
        return false;
    }
    let Some(uri) = find(&params, "uri") else {
        return false;
    };
    let Some(client_response) = find(&params, "response") else {
        return false;
    };

    let ha1 = ha1(username, realm, password);
    let ha2 = ha2(method, uri);
    let qop = find(&params, "qop");
    let nc = find(&params, "nc");
    let cnonce = find(&params, "cnonce");
    match response(&ha1, nonce, &ha2, qop, nc, cnonce) {
        Some(expected) => expected.eq_ignore_ascii_case(client_response),
        None => false,
    }
}

/// Build the `Authentication-Info` response header value (RFC 2617 §3.2.3). Strict legacy CWMP clients require it to keep an authenticated session alive rather than restarting.
pub fn authentication_info(
    client_auth_header: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    next_nonce: &str,
) -> Option<String> {
    let params = parse_digest_params(client_auth_header);
    let username = find(&params, "username")?;
    let uri = find(&params, "uri")?;
    let qop = find(&params, "qop");
    let nc = find(&params, "nc");
    let cnonce = find(&params, "cnonce");

    let ha1 = ha1(username, realm, password);
    // RFC 2617 §3.2.3: A2 for the response-auth digest omits the method. It
    // is `":" uri` rather than the request's `method ":" uri`.
    let ha2 = ha2("", uri);
    let rspauth = response(&ha1, nonce, &ha2, qop, nc, cnonce)?;

    let mut parts = Vec::new();
    if let Some(qop) = qop {
        parts.push(format!("qop={qop}"));
        parts.push(format!("rspauth=\"{rspauth}\""));
        if let Some(cnonce) = cnonce {
            parts.push(format!("cnonce=\"{cnonce}\""));
        }
        if let Some(nc) = nc {
            parts.push(format!("nc={nc}"));
        }
    } else {
        parts.push(format!("rspauth=\"{rspauth}\""));
    }
    parts.push(format!("nextnonce=\"{next_nonce}\""));
    Some(parts.join(", "))
}
