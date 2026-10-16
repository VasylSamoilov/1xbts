//! SIP Digest authentication (RFC 3261 §22, RFC 2617), MD5 with optional
//! `qop=auth`.

use md5::{Digest, Md5};

const SCHEME: &str = "Digest";
pub const ALGORITHM_MD5: &str = "MD5";
pub const QOP_AUTH: &str = "auth";

fn md5_hex(parts: &[&str]) -> String {
    let mut hasher = Md5::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hasher.update(b":");
        }
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn auth_param(value: &str, name: &str) -> Option<String> {
    let body = value
        .strip_prefix(SCHEME)
        .map(str::trim_start)
        .unwrap_or(value);

    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b',' {
            i += 1;
        }
        let key = body[key_start..i].trim();
        if i >= bytes.len() || bytes[i] == b',' {
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        i += 1; // consume '='

        let val = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let v = &body[start..i];
            if i < bytes.len() {
                i += 1; // consume closing quote
            }
            v.to_string()
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            body[start..i].trim().to_string()
        };

        if key.eq_ignore_ascii_case(name) {
            return Some(val);
        }

        while i < bytes.len() && bytes[i] != b',' {
            i += 1;
        }
        if i < bytes.len() {
            i += 1;
        }
    }
    None
}

fn ha1(username: &str, realm: &str, password: &str) -> String {
    md5_hex(&[username, realm, password])
}

fn ha2(method: &str, uri: &str) -> String {
    md5_hex(&[method, uri])
}

/// When `qop` is `Some`, the RFC 2617 qop=auth form is used and both `cnonce` and `nc` participate. Otherwise the RFC 2069 legacy form is used.
#[allow(clippy::too_many_arguments)]
fn digest_response(
    username: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    method: &str,
    uri: &str,
    qop: Option<&str>,
    cnonce: Option<&str>,
    nc: Option<&str>,
) -> String {
    let ha1 = ha1(username, realm, password);
    let ha2 = ha2(method, uri);
    match (qop, cnonce, nc) {
        (Some(qop), Some(cnonce), Some(nc)) => md5_hex(&[&ha1, nonce, nc, cnonce, qop, &ha2]),
        _ => md5_hex(&[&ha1, nonce, &ha2]),
    }
}

pub fn authentication_info(authorization_value: &str, password: &str) -> Option<String> {
    let username = auth_param(authorization_value, "username")?;
    let realm = auth_param(authorization_value, "realm")?;
    let nonce = auth_param(authorization_value, "nonce")?;
    let uri = auth_param(authorization_value, "uri")?;
    let qop = auth_param(authorization_value, "qop")?;
    let cnonce = auth_param(authorization_value, "cnonce")?;
    let nc = auth_param(authorization_value, "nc")?;
    if !qop.eq_ignore_ascii_case(QOP_AUTH) {
        return None;
    }
    let rspauth = digest_response(
        &username,
        password,
        &realm,
        &nonce,
        "",
        &uri,
        Some(&qop),
        Some(&cnonce),
        Some(&nc),
    );
    Some(format!(
        "qop={qop}, rspauth=\"{rspauth}\", cnonce=\"{cnonce}\", nc={nc}"
    ))
}

pub fn challenge(realm: &str, nonce: &str, opaque: Option<&str>) -> String {
    let mut value = format!(
        "{SCHEME} realm=\"{realm}\", nonce=\"{nonce}\", qop=\"{QOP_AUTH}\", algorithm={ALGORITHM_MD5}"
    );
    if let Some(opaque) = opaque {
        value.push_str(&format!(", opaque=\"{opaque}\""));
    }
    value
}

#[allow(clippy::too_many_arguments)]
pub fn build_authorization(
    username: &str,
    password: &str,
    realm: &str,
    nonce: &str,
    method: &str,
    uri: &str,
    qop: Option<&str>,
    cnonce: Option<&str>,
    nc: Option<&str>,
) -> String {
    let response = digest_response(
        username, password, realm, nonce, method, uri, qop, cnonce, nc,
    );
    let mut value = format!(
        "{SCHEME} username=\"{username}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\", algorithm={ALGORITHM_MD5}"
    );
    if let (Some(qop), Some(cnonce), Some(nc)) = (qop, cnonce, nc) {
        value.push_str(&format!(", qop={qop}, nc={nc}, cnonce=\"{cnonce}\""));
    }
    value
}

pub fn verify(
    authorization_value: &str,
    method: &str,
    expected_password: &str,
    expected_uri: Option<&str>,
) -> bool {
    let username = match auth_param(authorization_value, "username") {
        Some(v) => v,
        None => return false,
    };
    let realm = match auth_param(authorization_value, "realm") {
        Some(v) => v,
        None => return false,
    };
    let nonce = match auth_param(authorization_value, "nonce") {
        Some(v) => v,
        None => return false,
    };
    let uri = match auth_param(authorization_value, "uri") {
        Some(v) => v,
        None => return false,
    };
    let client_response = match auth_param(authorization_value, "response") {
        Some(v) => v,
        None => return false,
    };

    if let Some(expected_uri) = expected_uri {
        if expected_uri != uri {
            return false;
        }
    }

    let qop = auth_param(authorization_value, "qop");
    let cnonce = auth_param(authorization_value, "cnonce");
    let nc = auth_param(authorization_value, "nc");

    let expected = digest_response(
        &username,
        expected_password,
        &realm,
        &nonce,
        method,
        &uri,
        qop.as_deref(),
        cnonce.as_deref(),
        nc.as_deref(),
    );

    // The Digest response is a hex string, so a case-insensitive compare
    // tolerates clients that upper-case it.
    expected.eq_ignore_ascii_case(&client_response)
}

pub fn authorization_username(authorization_value: &str) -> Option<String> {
    auth_param(authorization_value, "username")
}
