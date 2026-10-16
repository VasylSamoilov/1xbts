use super::*;
use crate::digest::{DigestParams, QOP_AUTH, digest_authorization, verify_digest};

#[test]
fn inform_response_round_trip() {
    let msg = CwmpMessage::InformResponse(InformResponse { max_envelopes: 1 });
    let xml = write_envelope(&msg, "42").unwrap();
    assert!(xml.contains("InformResponse"));
    assert!(xml.contains("cwmp:ID"));
    assert!(xml.contains("<SOAP-ENV:Envelope"));
    assert!(xml.contains("<SOAP-ENV:Body>"));

    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::InformResponse(r) => assert_eq!(r.max_envelopes, 1),
        other => panic!("expected InformResponse, got {other:?}"),
    }
}

#[test]
fn legacy_inform_response_echoes_id_and_uses_legacy_encoding_namespace() {
    let xml = write_legacy_inform_response(&InformResponse { max_envelopes: 1 }, "190");

    assert!(xml.contains("<cwmp:InformResponse>"));
    assert!(xml.contains("<MaxEnvelopes>1</MaxEnvelopes>"));
    assert!(xml.contains("<SOAP-ENV:Header>"));
    assert!(xml.contains(">190</cwmp:ID>"));
    assert!(xml.contains("soap/encoding\""));
    assert!(!xml.contains("soap/encoding/\""));
}

#[test]
fn set_parameter_values_round_trip() {
    let msg = CwmpMessage::SetParameterValues(SetParameterValues {
        parameter_list: vec![
            ParameterValue::new(
                "Device.Services.FAPService.1.CellConfig.CDMA2000.OneX.RAN.SectorID",
                "12345",
            ),
            ParameterValue::typed(
                "Device.Services.FAPService.1.CellConfig.CDMA2000.OneX.RAN.MaxFAPTxPower",
                "24",
                "xsd:int",
            ),
        ],
        parameter_key: "provision-2026-08-26".to_string(),
    });

    let xml = write_envelope(&msg, "cpe-7").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::SetParameterValues(s) => {
            assert_eq!(s.parameter_key, "provision-2026-08-26");
            assert_eq!(s.parameter_list.len(), 2);
            assert_eq!(s.parameter_list[0].value, "12345");
            assert_eq!(s.parameter_list[0].xsi_type, "xsd:string");
            assert_eq!(s.parameter_list[1].xsi_type, "xsd:int");
            assert_eq!(s.parameter_list[1].value, "24");
        }
        other => panic!("expected SetParameterValues, got {other:?}"),
    }
}

#[test]
fn set_parameter_values_response_round_trip() {
    let msg = CwmpMessage::SetParameterValuesResponse(SetParameterValuesResponse { status: 1 });
    let xml = write_envelope(&msg, "9").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::SetParameterValuesResponse(r) => assert_eq!(r.status, 1),
        other => panic!("expected SetParameterValuesResponse, got {other:?}"),
    }
}

#[test]
fn add_object_round_trip() {
    let msg = CwmpMessage::AddObject(AddObject {
        object_name: "InternetGatewayDevice.Services.UserProfile.".to_string(),
        parameter_key: "1xbts-femto".to_string(),
    });
    let xml = write_envelope(&msg, "10").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::AddObject(request) => {
            assert_eq!(
                request.object_name,
                "InternetGatewayDevice.Services.UserProfile."
            );
            assert_eq!(request.parameter_key, "1xbts-femto");
        }
        other => panic!("expected AddObject, got {other:?}"),
    }
}

#[test]
fn add_object_response_round_trip() {
    let msg = CwmpMessage::AddObjectResponse(AddObjectResponse {
        instance_number: 0,
        status: 1,
    });
    let xml = write_envelope(&msg, "11").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::AddObjectResponse(response) => {
            assert_eq!(response.instance_number, 0);
            assert_eq!(response.status, 1);
        }
        other => panic!("expected AddObjectResponse, got {other:?}"),
    }
}

#[test]
fn delete_object_round_trip() {
    let msg = CwmpMessage::DeleteObject(DeleteObject {
        object_name: "InternetGatewayDevice.Services.UserProfile.2.".to_string(),
        parameter_key: "1xbts-femto".to_string(),
    });
    let xml = write_envelope(&msg, "12").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::DeleteObject(request) => {
            assert_eq!(
                request.object_name,
                "InternetGatewayDevice.Services.UserProfile.2."
            );
            assert_eq!(request.parameter_key, "1xbts-femto");
        }
        other => panic!("expected DeleteObject, got {other:?}"),
    }
}

#[test]
fn delete_object_response_round_trip() {
    let msg = CwmpMessage::DeleteObjectResponse(DeleteObjectResponse { status: 1 });
    let xml = write_envelope(&msg, "13").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::DeleteObjectResponse(response) => assert_eq!(response.status, 1),
        other => panic!("expected DeleteObjectResponse, got {other:?}"),
    }
}

#[test]
fn decode_realistic_inform() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"
  xmlns:soap-enc="http://schemas.xmlsoap.org/soap/encoding/"
  xmlns:xsd="http://www.w3.org/2001/XMLSchema"
  xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
  xmlns:cwmp="urn:dslforum-org:cwmp-1-2">
  <soap:Header><cwmp:ID soap:mustUnderstand="1">1001</cwmp:ID></soap:Header>
  <soap:Body>
    <cwmp:Inform>
      <DeviceId>
        <Manufacturer>Airvana</Manufacturer>
        <OUI>0005B9</OUI>
        <ProductClass>cdmaFAP</ProductClass>
        <SerialNumber>0005B9AABBCC</SerialNumber>
      </DeviceId>
      <Event soap-enc:arrayType="cwmp:EventStruct[2]">
        <EventStruct><EventCode>0 BOOTSTRAP</EventCode><CommandKey></CommandKey></EventStruct>
        <EventStruct><EventCode>1 BOOT</EventCode><CommandKey></CommandKey></EventStruct>
      </Event>
      <MaxEnvelopes>1</MaxEnvelopes>
      <CurrentTime>2026-08-26T12:00:00Z</CurrentTime>
      <RetryCount>0</RetryCount>
      <ParameterList soap-enc:arrayType="cwmp:ParameterValueStruct[2]">
        <ParameterValueStruct>
          <Name>Device.DeviceInfo.HardwareVersion</Name>
          <Value xsi:type="xsd:string">1.0</Value>
        </ParameterValueStruct>
        <ParameterValueStruct>
          <Name>Device.Services.FAPService.1.FAPControl.CDMA2000.OneX.OpState</Name>
          <Value xsi:type="xsd:boolean">true</Value>
        </ParameterValueStruct>
      </ParameterList>
    </cwmp:Inform>
  </soap:Body>
</soap:Envelope>"#;

    assert_eq!(
        parse_envelope_id(xml.as_bytes()).unwrap().as_deref(),
        Some("1001")
    );
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::Inform(inform) => {
            assert_eq!(inform.device_id.manufacturer, "Airvana");
            assert_eq!(inform.device_id.oui, "0005B9");
            assert_eq!(inform.device_id.product_class, "cdmaFAP");
            assert_eq!(inform.device_id.serial_number, "0005B9AABBCC");
            assert_eq!(inform.max_envelopes, 1);
            assert_eq!(inform.retry_count, 0);
            assert_eq!(inform.current_time, "2026-08-26T12:00:00Z");

            assert_eq!(inform.events.len(), 2);
            assert_eq!(inform.events[0].event_code, "0 BOOTSTRAP");
            assert_eq!(inform.events[1].event_code, "1 BOOT");

            assert_eq!(inform.parameter_list.len(), 2);
            assert_eq!(
                inform.parameter_list[0].name,
                "Device.DeviceInfo.HardwareVersion"
            );
            assert_eq!(inform.parameter_list[0].value, "1.0");
            assert_eq!(inform.parameter_list[0].xsi_type, "xsd:string");
            assert_eq!(inform.parameter_list[1].value, "true");
            assert_eq!(inform.parameter_list[1].xsi_type, "xsd:boolean");
        }
        other => panic!("expected Inform, got {other:?}"),
    }
}

#[test]
fn get_parameter_values_round_trip() {
    let msg = CwmpMessage::GetParameterValues(GetParameterValues {
        parameter_names: vec![
            "Device.DeviceInfo.".to_string(),
            "Device.Services.FAPService.1.".to_string(),
        ],
    });
    let xml = write_envelope(&msg, "3").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::GetParameterValues(g) => {
            assert_eq!(g.parameter_names.len(), 2);
            assert_eq!(g.parameter_names[0], "Device.DeviceInfo.");
        }
        other => panic!("expected GetParameterValues, got {other:?}"),
    }
}

#[test]
fn get_parameter_names_response_round_trip() {
    let msg = CwmpMessage::GetParameterNamesResponse(GetParameterNamesResponse {
        parameters: vec![
            ParameterInfo {
                name: "Device.DeviceInfo.HardwareVersion".to_string(),
                writable: false,
            },
            ParameterInfo {
                name: "Device.ManagementServer.PeriodicInformInterval".to_string(),
                writable: true,
            },
        ],
    });
    let xml = write_envelope(&msg, "5").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::GetParameterNamesResponse(r) => {
            assert_eq!(r.parameters.len(), 2);
            assert!(!r.parameters[0].writable);
            assert!(r.parameters[1].writable);
        }
        other => panic!("expected GetParameterNamesResponse, got {other:?}"),
    }
}

#[test]
fn get_rpc_methods_response_round_trip() {
    let msg = CwmpMessage::GetRpcMethodsResponse(GetRpcMethodsResponse {
        method_list: vec![
            "Inform".to_string(),
            "GetParameterValues".to_string(),
            "SetParameterValues".to_string(),
        ],
    });
    let xml = write_envelope(&msg, "6").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::GetRpcMethodsResponse(r) => {
            assert_eq!(r.method_list.len(), 3);
            assert_eq!(r.method_list[1], "GetParameterValues");
        }
        other => panic!("expected GetRPCMethodsResponse, got {other:?}"),
    }
}

#[test]
fn fault_round_trip() {
    let msg = CwmpMessage::Fault(Fault {
        soap_fault_code: "Client".to_string(),
        soap_fault_string: "CWMP fault".to_string(),
        fault_code: "9001".to_string(),
        fault_string: "Request denied".to_string(),
        parameter_faults: vec![ParameterFault {
            parameter_name: "Device.Radio.Admin".to_string(),
            fault_code: "9006".to_string(),
            fault_string: "Invalid parameter type".to_string(),
        }],
    });
    let xml = write_envelope(&msg, "7").unwrap();
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::Fault(f) => {
            assert_eq!(f.fault_code, "9001");
            assert_eq!(f.fault_string, "Request denied");
            assert_eq!(f.soap_fault_code, "Client");
            assert_eq!(f.parameter_faults.len(), 1);
            assert_eq!(f.parameter_faults[0].parameter_name, "Device.Radio.Admin");
        }
        other => panic!("expected Fault, got {other:?}"),
    }
}

#[test]
fn empty_post_is_sentinel() {
    assert_eq!(parse_envelope(b"").unwrap(), CwmpMessage::Empty);
    assert_eq!(parse_envelope(b"   \n").unwrap(), CwmpMessage::Empty);
    assert_eq!(write_envelope(&CwmpMessage::Empty, "1").unwrap(), "");
}

#[test]
fn xml_special_chars_escape_and_round_trip() {
    let msg = CwmpMessage::SetParameterValues(SetParameterValues {
        parameter_list: vec![ParameterValue::new("Device.Note", "a & b < c > d \"x\"")],
        parameter_key: "k<&>".to_string(),
    });
    let xml = write_envelope(&msg, "8").unwrap();
    assert!(xml.contains("&amp;"));
    match parse_envelope(xml.as_bytes()).unwrap() {
        CwmpMessage::SetParameterValues(s) => {
            assert_eq!(s.parameter_list[0].value, "a & b < c > d \"x\"");
            assert_eq!(s.parameter_key, "k<&>");
        }
        other => panic!("expected SetParameterValues, got {other:?}"),
    }
}

#[test]
fn digest_auth_round_trips_and_rejects_wrong_password() {
    let params = DigestParams {
        username: "fap0005B9AABBCC",
        password: "s3cr3t-provision-key",
        realm: "1xbts-acs",
        nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093",
        method: "POST",
        uri: "/acs",
        qop: Some(QOP_AUTH),
        cnonce: Some("0a4f113b"),
        nc: Some("00000001"),
        opaque: Some("5ccc069c403ebaf9f0171e9517f40e41"),
    };
    let header = digest_authorization(&params);
    assert!(header.starts_with("Digest "));
    assert!(header.contains("qop=auth"));
    assert!(header.contains("algorithm=MD5"));

    assert!(verify_digest(
        &header,
        "fap0005B9AABBCC",
        "s3cr3t-provision-key",
        "1xbts-acs",
        "dcd98b7102dd2f0e8b11d0f600bfb0c093",
        "POST",
    ));

    assert!(!verify_digest(
        &header,
        "fap0005B9AABBCC",
        "wrong-password",
        "1xbts-acs",
        "dcd98b7102dd2f0e8b11d0f600bfb0c093",
        "POST",
    ));

    assert!(!verify_digest(
        &header,
        "someone-else",
        "s3cr3t-provision-key",
        "1xbts-acs",
        "dcd98b7102dd2f0e8b11d0f600bfb0c093",
        "POST",
    ));
}

#[test]
fn digest_auth_qopless_round_trips() {
    let params = DigestParams {
        username: "cpe",
        password: "pw",
        realm: "acs",
        nonce: "abc123",
        method: "POST",
        uri: "/acs",
        qop: None,
        cnonce: None,
        nc: None,
        opaque: None,
    };
    let header = digest_authorization(&params);
    assert!(!header.contains("qop"));
    assert!(verify_digest(&header, "cpe", "pw", "acs", "abc123", "POST"));
    assert!(!verify_digest(&header, "cpe", "pw", "acs", "abc123", "GET"));
}
