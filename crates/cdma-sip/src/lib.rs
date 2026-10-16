//! A self-contained SIP message layer for the femtocell IMS core (P/I/S-CSCF
//! and registrar) and the FCS request relay.

pub mod auth;
pub mod message;
pub mod registrar;
pub mod transport;

pub use message::{
    Headers, Method, SipError, SipMessage, SipRequest, SipResponse, parse, serialize,
};
pub use registrar::{Binding, Registrar, challenge_response, response};
pub use transport::{TransportError, UdpSipTransport};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{
        H_CONTACT, H_CONTENT_TYPE, H_CSEQ, H_EXPIRES, H_VIA, SipMessage, param_value,
    };

    const REGISTER_WIRE: &str = concat!(
        "REGISTER sip:ims.example.net SIP/2.0\r\n",
        "Via: SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bKnashds7\r\n",
        "Max-Forwards: 70\r\n",
        "From: <sip:fap-1@ims.example.net>;tag=abc123\r\n",
        "To: <sip:fap-1@ims.example.net>\r\n",
        "Call-ID: 843817637684230@10.0.0.5\r\n",
        "CSeq: 1 REGISTER\r\n",
        "Contact: <sip:fap-1@10.0.0.5:5060>\r\n",
        "Expires: 3600\r\n",
        "Content-Length: 0\r\n",
        "\r\n",
    );

    fn register_request() -> SipRequest {
        match SipMessage::parse(REGISTER_WIRE.as_bytes()).unwrap() {
            SipMessage::Request(r) => r,
            other => panic!("expected request, got {other:?}"),
        }
    }

    #[test]
    fn parses_and_reserializes_register() {
        let req = register_request();
        assert_eq!(req.method, Method::Register);
        assert_eq!(req.uri, "sip:ims.example.net");
        assert_eq!(req.version, "SIP/2.0");
        assert_eq!(req.headers.via_branch().as_deref(), Some("z9hG4bKnashds7"));
        assert_eq!(req.headers.from_tag().as_deref(), Some("abc123"));
        assert_eq!(req.headers.call_id(), Some("843817637684230@10.0.0.5"));
        assert_eq!(req.headers.cseq(), Some((1, "REGISTER".to_string())));
        assert_eq!(req.headers.expires(), Some(3600));
        assert_eq!(req.headers.max_forwards(), Some(70));
        assert!(req.headers.contact().is_some());

        let bytes = SipMessage::Request(req.clone()).serialize();
        let reparsed = match SipMessage::parse(&bytes).unwrap() {
            SipMessage::Request(r) => r,
            other => panic!("expected request, got {other:?}"),
        };
        assert_eq!(reparsed.method, req.method);
        assert_eq!(reparsed.uri, req.uri);
        assert_eq!(reparsed.headers.via(), req.headers.via());
        assert_eq!(reparsed.headers.from(), req.headers.from());
        assert_eq!(reparsed.headers.to(), req.headers.to());
        assert_eq!(reparsed.headers.call_id(), req.headers.call_id());
        assert_eq!(reparsed.headers.cseq(), req.headers.cseq());
        assert_eq!(reparsed.headers.content_length(), Some(0));
    }

    #[test]
    fn parses_200_ok() {
        let wire = concat!(
            "SIP/2.0 200 OK\r\n",
            "Via: SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bKnashds7\r\n",
            "From: <sip:fap-1@ims.example.net>;tag=abc123\r\n",
            "To: <sip:fap-1@ims.example.net>;tag=svr99\r\n",
            "Call-ID: 843817637684230@10.0.0.5\r\n",
            "CSeq: 1 REGISTER\r\n",
            "Contact: <sip:fap-1@10.0.0.5:5060>;expires=3600\r\n",
            "Content-Length: 0\r\n",
            "\r\n",
        );
        let resp = match SipMessage::parse(wire.as_bytes()).unwrap() {
            SipMessage::Response(r) => r,
            other => panic!("expected response, got {other:?}"),
        };
        assert_eq!(resp.status, 200);
        assert_eq!(resp.reason, "OK");
        assert_eq!(resp.headers.to_tag().as_deref(), Some("svr99"));
    }

    #[test]
    fn parses_401_with_www_authenticate() {
        let wire = concat!(
            "SIP/2.0 401 Unauthorized\r\n",
            "Via: SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bKnashds7\r\n",
            "From: <sip:fap-1@ims.example.net>;tag=abc123\r\n",
            "To: <sip:fap-1@ims.example.net>;tag=svr99\r\n",
            "Call-ID: 843817637684230@10.0.0.5\r\n",
            "CSeq: 1 REGISTER\r\n",
            "WWW-Authenticate: Digest realm=\"ims.example.net\", ",
            "nonce=\"dcd98b7102dd2f0e\", qop=\"auth\", algorithm=MD5\r\n",
            "Content-Length: 0\r\n",
            "\r\n",
        );
        let resp = match SipMessage::parse(wire.as_bytes()).unwrap() {
            SipMessage::Response(r) => r,
            other => panic!("expected response, got {other:?}"),
        };
        assert_eq!(resp.status, 401);
        let wa = resp.headers.www_authenticate().unwrap();
        assert_eq!(
            auth::auth_param(wa, "realm").as_deref(),
            Some("ims.example.net")
        );
        assert_eq!(
            auth::auth_param(wa, "nonce").as_deref(),
            Some("dcd98b7102dd2f0e")
        );
    }

    #[test]
    fn parses_message_with_body() {
        let body = "Hello from the FAP";
        let wire = format!(
            concat!(
                "MESSAGE sip:user@ims.example.net SIP/2.0\r\n",
                "Via: SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bKmsg\r\n",
                "From: <sip:fap-1@ims.example.net>;tag=t1\r\n",
                "To: <sip:user@ims.example.net>\r\n",
                "Call-ID: msg-call-1@10.0.0.5\r\n",
                "CSeq: 5 MESSAGE\r\n",
                "Content-Type: text/plain\r\n",
                "Content-Length: {}\r\n",
                "\r\n",
                "{}",
            ),
            body.len(),
            body,
        );
        let req = match SipMessage::parse(wire.as_bytes()).unwrap() {
            SipMessage::Request(r) => r,
            other => panic!("expected request, got {other:?}"),
        };
        assert_eq!(req.method, Method::Message);
        assert_eq!(req.headers.get(H_CONTENT_TYPE), Some("text/plain"));
        assert_eq!(req.body, body.as_bytes());
    }

    #[test]
    fn digest_round_trips_and_rejects_wrong_password() {
        let realm = "ims.example.net";
        let nonce = "dcd98b7102dd2f0e";
        let method = "REGISTER";
        let uri = "sip:ims.example.net";
        let user = "fap-1";
        let password = "s3cret";
        let cnonce = "0a4f113b";
        let nc = "00000001";

        let challenge = auth::challenge(realm, nonce, None);
        assert_eq!(
            auth::auth_param(&challenge, "realm").as_deref(),
            Some(realm)
        );

        let authz = auth::build_authorization(
            user,
            password,
            realm,
            nonce,
            method,
            uri,
            Some(auth::QOP_AUTH),
            Some(cnonce),
            Some(nc),
        );
        assert!(auth::verify(&authz, method, password, Some(uri)));
        assert!(!auth::verify(&authz, method, "wrong-password", Some(uri)));
        assert!(!auth::verify(&authz, "INVITE", password, Some(uri)));

        let authz_legacy =
            auth::build_authorization(user, password, realm, nonce, method, uri, None, None, None);
        assert!(auth::verify(&authz_legacy, method, password, Some(uri)));
    }

    #[test]
    fn registrar_stores_refreshes_and_deregisters() {
        let mut reg = Registrar::new();
        let req = register_request();

        let binding = reg.register(&req).expect("binding stored");
        assert_eq!(binding.expires, 3600);
        assert_eq!(reg.len(), 1);
        let ok = reg.ok_response(&req);
        assert_eq!(ok.status, registrar::SC_OK);
        assert!(ok.headers.contact().is_some());
        assert_eq!(ok.headers.expires(), None);
        assert_eq!(
            ok.headers.contact(),
            Some("<sip:fap-1@10.0.0.5:5060>;expires=3600")
        );

        let mut refresh = req.clone();
        refresh.headers.set(H_EXPIRES, "1800");
        let refreshed = reg.register(&refresh).expect("binding refreshed");
        assert_eq!(refreshed.expires, 1800);
        assert_eq!(reg.len(), 1);

        let mut dereg = req.clone();
        dereg.headers.set(H_EXPIRES, "0");
        assert!(reg.register(&dereg).is_none());
        assert_eq!(reg.len(), 0);
        let ok = reg.ok_response(&dereg);
        assert_eq!(ok.headers.expires(), None);
    }

    #[test]
    fn registrar_echoes_contact_with_one_expiry() {
        let mut registrar = Registrar::new();
        let mut request = register_request();
        request.headers.set(
            H_CONTACT,
            "0005B9AABBCC<sip:0005B9AABBCC@198.51.100.1:5060>;vendor=airvana;expires=120",
        );

        registrar.register(&request);
        let response = registrar.ok_response(&request);

        assert_eq!(
            response.headers.contact(),
            Some("0005B9AABBCC<sip:0005B9AABBCC@198.51.100.1:5060>;vendor=airvana;expires=120")
        );
    }

    #[test]
    fn response_preserves_dialog_and_adds_to_tag() {
        let req = register_request();
        let resp = response(&req, registrar::SC_OK, "OK");
        assert_eq!(resp.headers.via(), req.headers.via());
        assert_eq!(resp.headers.get_all(H_VIA).len(), 1);
        assert_eq!(resp.headers.call_id(), req.headers.call_id());
        assert_eq!(resp.headers.cseq(), req.headers.cseq());
        assert_eq!(resp.headers.from(), req.headers.from());
        assert!(req.headers.to_tag().is_none());
        assert!(resp.headers.to_tag().is_some());
        assert_eq!(
            param_value(resp.headers.to().unwrap(), "tag"),
            resp.headers.to_tag()
        );
    }

    #[test]
    fn response_reuses_to_tag_for_registration_refreshes() {
        let request = register_request();
        let first = response(&request, registrar::SC_OK, "OK");
        let mut refresh = request.clone();
        refresh.headers.set(H_CSEQ, "2 REGISTER");
        let second = response(&refresh, registrar::SC_OK, "OK");

        assert_eq!(first.headers.to_tag(), second.headers.to_tag());
    }

    #[test]
    fn challenge_response_carries_www_authenticate() {
        let req = register_request();
        let resp = challenge_response(&req, "ims.example.net", "nonce-xyz");
        assert_eq!(resp.status, registrar::SC_UNAUTHORIZED);
        let wa = resp.headers.www_authenticate().unwrap();
        assert_eq!(auth::auth_param(wa, "nonce").as_deref(), Some("nonce-xyz"));
    }

    #[tokio::test]
    async fn udp_transport_round_trip() {
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};

        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let mut server = UdpSipTransport::bind(loopback).await.unwrap();
        let client = UdpSipTransport::bind(loopback).await.unwrap();
        let server_addr = server.local_addr().unwrap();

        let msg = SipMessage::parse(REGISTER_WIRE.as_bytes()).unwrap();
        client.send(&msg, server_addr).await.unwrap();

        let (received, from) = server.recv().await.unwrap();
        assert_eq!(from, client.local_addr().unwrap());
        match received {
            SipMessage::Request(r) => {
                assert_eq!(r.method, Method::Register);
                assert_eq!(r.headers.call_id(), msg.headers().call_id());
            }
            other => panic!("expected request, got {other:?}"),
        }
    }
}
