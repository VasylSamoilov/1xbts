//! TR-069 / CWMP (CPE WAN Management Protocol) SOAP message codec.

pub mod digest;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::escape::{resolve_predefined_entity, unescape};
use quick_xml::events::Event;
use quick_xml::name::QName;

const SOAP_ENV_NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const SOAP_ENC_NS: &str = "http://schemas.xmlsoap.org/soap/encoding/";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema";
const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";
/// CWMP 1.0 namespace this crate emits. Decoding also accepts -1-1 and -1-2.
const CWMP_NS: &str = "urn:dslforum-org:cwmp-1-0";

const P_ENV: &str = "SOAP-ENV";
const P_ENC: &str = "SOAP-ENC";
const P_CWMP: &str = "cwmp";

const DEFAULT_XSI_TYPE: &str = "xsd:string";

#[derive(Debug, thiserror::Error)]
pub enum CwmpError {
    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("XML decode error: {0}")]
    Decode(String),
    #[error("invalid UTF-8 in XML: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("not a SOAP envelope: {0}")]
    NotEnvelope(&'static str),
    #[error("unsupported or unrecognized CWMP RPC: {0}")]
    UnknownRpc(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CwmpMessage {
    Inform(Inform),
    InformResponse(InformResponse),
    GetParameterNames(GetParameterNames),
    GetParameterNamesResponse(GetParameterNamesResponse),
    GetParameterValues(GetParameterValues),
    GetParameterValuesResponse(GetParameterValuesResponse),
    SetParameterValues(SetParameterValues),
    SetParameterValuesResponse(SetParameterValuesResponse),
    AddObject(AddObject),
    AddObjectResponse(AddObjectResponse),
    DeleteObject(DeleteObject),
    DeleteObjectResponse(DeleteObjectResponse),
    GetRpcMethods(GetRpcMethods),
    GetRpcMethodsResponse(GetRpcMethodsResponse),
    Reboot(Reboot),
    RebootResponse(RebootResponse),
    Fault(Fault),
    /// A zero-length HTTP POST body: the CPE signalling it has nothing more to
    /// send in this session.
    Empty,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceId {
    pub manufacturer: String,
    pub oui: String,
    pub product_class: String,
    pub serial_number: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventStruct {
    pub event_code: String,
    pub command_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParameterValue {
    pub name: String,
    pub value: String,
    pub xsi_type: String,
}

impl ParameterValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            xsi_type: DEFAULT_XSI_TYPE.to_string(),
        }
    }

    pub fn typed(
        name: impl Into<String>,
        value: impl Into<String>,
        xsi_type: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            xsi_type: xsi_type.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParameterInfo {
    pub name: String,
    pub writable: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inform {
    pub device_id: DeviceId,
    pub events: Vec<EventStruct>,
    pub max_envelopes: u32,
    pub current_time: String,
    pub retry_count: u32,
    pub parameter_list: Vec<ParameterValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InformResponse {
    pub max_envelopes: u32,
}

impl Default for InformResponse {
    fn default() -> Self {
        Self { max_envelopes: 1 }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetParameterNames {
    pub parameter_path: String,
    pub next_level: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetParameterNamesResponse {
    pub parameters: Vec<ParameterInfo>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetParameterValues {
    pub parameter_names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetParameterValuesResponse {
    pub parameters: Vec<ParameterValue>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetParameterValues {
    pub parameter_list: Vec<ParameterValue>,
    pub parameter_key: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetParameterValuesResponse {
    /// 0 = applied, 1 = applied after a reboot (per TR-069).
    pub status: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AddObject {
    pub object_name: String,
    pub parameter_key: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AddObjectResponse {
    pub instance_number: u32,
    /// 0 = applied, 1 = applied after a reboot (per TR-069).
    pub status: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeleteObject {
    pub object_name: String,
    pub parameter_key: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeleteObjectResponse {
    /// 0 = applied, 1 = applied after a reboot (per TR-069).
    pub status: i32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetRpcMethods;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GetRpcMethodsResponse {
    pub method_list: Vec<String>,
}

/// ACS-initiated CPE reboot (TR-069 §A.3.2.8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reboot {
    pub command_key: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RebootResponse;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fault {
    pub soap_fault_code: String,
    pub soap_fault_string: String,
    pub fault_code: String,
    pub fault_string: String,
    pub parameter_faults: Vec<ParameterFault>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParameterFault {
    pub parameter_name: String,
    pub fault_code: String,
    pub fault_string: String,
}

#[derive(Debug, Default)]
struct Node {
    local: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn child(&self, local: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.local == local)
    }

    fn children_named<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.local == local)
    }

    fn text_of(&self, local: &str) -> Option<String> {
        self.child(local).map(|c| c.text.clone())
    }

    fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == local)
            .map(|(_, v)| v.as_str())
    }
}

fn local_name(qname: QName<'_>) -> String {
    String::from_utf8_lossy(qname.local_name().as_ref()).into_owned()
}

fn build_dom(xml: &str) -> Result<Node, CwmpError> {
    // Untrimmed so leaf values keep their exact content.
    let mut reader = Reader::from_str(xml);

    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let mut node = Node {
                    local: local_name(e.name()),
                    ..Node::default()
                };
                collect_attrs(&e, &mut node)?;
                stack.push(node);
            }
            Event::Empty(e) => {
                let mut node = Node {
                    local: local_name(e.name()),
                    ..Node::default()
                };
                collect_attrs(&e, &mut node)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                }
            }
            Event::End(_) => {
                if stack.len() > 1 {
                    let node = stack.pop().expect("stack has >1 element");
                    stack
                        .last_mut()
                        .expect("root sentinel is always present")
                        .children
                        .push(node);
                }
            }
            Event::Text(e) => {
                let raw = e
                    .decode()
                    .map_err(|err| CwmpError::Decode(err.to_string()))?;
                let text = unescape(&raw).map_err(|err| CwmpError::Decode(err.to_string()))?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&text);
                }
            }
            Event::CData(e) => {
                if let Some(top) = stack.last_mut() {
                    let text = e
                        .decode()
                        .map_err(|err| CwmpError::Decode(err.to_string()))?;
                    top.text.push_str(&text);
                }
            }
            Event::GeneralRef(e) => {
                let resolved = if let Some(ch) = e
                    .resolve_char_ref()
                    .map_err(|err| CwmpError::Decode(err.to_string()))?
                {
                    ch.to_string()
                } else if let Some(text) = resolve_predefined_entity(
                    &e.decode()
                        .map_err(|err| CwmpError::Decode(err.to_string()))?,
                ) {
                    text.to_string()
                } else {
                    format!(
                        "&{};",
                        e.decode()
                            .map_err(|err| CwmpError::Decode(err.to_string()))?
                    )
                };
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&resolved);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let mut root = stack.pop().expect("root sentinel is always present");
    root.children
        .pop()
        .ok_or(CwmpError::NotEnvelope("no root element"))
}

fn collect_attrs(e: &quick_xml::events::BytesStart, node: &mut Node) -> Result<(), CwmpError> {
    for attr in e.attributes() {
        let attr = attr.map_err(|err| CwmpError::Decode(err.to_string()))?;
        let key = local_name(attr.key);
        let value = attr.normalized_value(XmlVersion::Implicit1_0)?.into_owned();
        node.attrs.push((key, value));
    }
    Ok(())
}

fn parse_bool(s: &str) -> bool {
    matches!(s.trim(), "1" | "true" | "TRUE" | "True")
}

fn parse_u32(node: &Node, local: &str) -> u32 {
    node.text_of(local)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub fn parse_envelope(xml: &[u8]) -> Result<CwmpMessage, CwmpError> {
    if xml.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(CwmpMessage::Empty);
    }

    let xml = std::str::from_utf8(xml)?;
    let envelope = build_dom(xml)?;
    if envelope.local != "Envelope" {
        return Err(CwmpError::NotEnvelope("root is not soap:Envelope"));
    }
    let body = envelope
        .child("Body")
        .ok_or(CwmpError::NotEnvelope("no soap:Body"))?;

    // A SOAP Fault sits directly under Body, not wrapped in a cwmp RPC.
    if let Some(fault) = body.child("Fault") {
        return Ok(CwmpMessage::Fault(parse_fault(fault)));
    }

    let Some(rpc) = body.children.first() else {
        return Ok(CwmpMessage::Empty);
    };

    match rpc.local.as_str() {
        "Inform" => Ok(CwmpMessage::Inform(parse_inform(rpc))),
        "InformResponse" => Ok(CwmpMessage::InformResponse(InformResponse {
            max_envelopes: parse_u32(rpc, "MaxEnvelopes"),
        })),
        "GetParameterNames" => Ok(CwmpMessage::GetParameterNames(GetParameterNames {
            parameter_path: rpc.text_of("ParameterPath").unwrap_or_default(),
            next_level: rpc
                .text_of("NextLevel")
                .map(|s| parse_bool(&s))
                .unwrap_or(false),
        })),
        "GetParameterNamesResponse" => Ok(CwmpMessage::GetParameterNamesResponse(
            parse_get_parameter_names_response(rpc),
        )),
        "GetParameterValues" => Ok(CwmpMessage::GetParameterValues(GetParameterValues {
            parameter_names: parse_string_list(rpc.child("ParameterNames")),
        })),
        "GetParameterValuesResponse" => Ok(CwmpMessage::GetParameterValuesResponse(
            GetParameterValuesResponse {
                parameters: parse_value_list(rpc.child("ParameterList")),
            },
        )),
        "SetParameterValues" => Ok(CwmpMessage::SetParameterValues(SetParameterValues {
            parameter_list: parse_value_list(rpc.child("ParameterList")),
            parameter_key: rpc.text_of("ParameterKey").unwrap_or_default(),
        })),
        "SetParameterValuesResponse" => Ok(CwmpMessage::SetParameterValuesResponse(
            SetParameterValuesResponse {
                status: rpc
                    .text_of("Status")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0),
            },
        )),
        "AddObject" => Ok(CwmpMessage::AddObject(AddObject {
            object_name: rpc.text_of("ObjectName").unwrap_or_default(),
            parameter_key: rpc.text_of("ParameterKey").unwrap_or_default(),
        })),
        "AddObjectResponse" => Ok(CwmpMessage::AddObjectResponse(AddObjectResponse {
            instance_number: parse_u32(rpc, "InstanceNumber"),
            status: rpc
                .text_of("Status")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
        })),
        "DeleteObject" => Ok(CwmpMessage::DeleteObject(DeleteObject {
            object_name: rpc.text_of("ObjectName").unwrap_or_default(),
            parameter_key: rpc.text_of("ParameterKey").unwrap_or_default(),
        })),
        "DeleteObjectResponse" => Ok(CwmpMessage::DeleteObjectResponse(DeleteObjectResponse {
            status: rpc
                .text_of("Status")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
        })),
        "GetRPCMethods" => Ok(CwmpMessage::GetRpcMethods(GetRpcMethods)),
        "GetRPCMethodsResponse" => Ok(CwmpMessage::GetRpcMethodsResponse(GetRpcMethodsResponse {
            method_list: parse_string_list(rpc.child("MethodList")),
        })),
        "Reboot" => Ok(CwmpMessage::Reboot(Reboot {
            command_key: rpc.text_of("CommandKey").unwrap_or_default(),
        })),
        "RebootResponse" => Ok(CwmpMessage::RebootResponse(RebootResponse)),
        other => Err(CwmpError::UnknownRpc(other.to_string())),
    }
}

pub fn parse_envelope_id(xml: &[u8]) -> Result<Option<String>, CwmpError> {
    if xml.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(None);
    }

    let xml = std::str::from_utf8(xml)?;
    let envelope = build_dom(xml)?;
    if envelope.local != "Envelope" {
        return Err(CwmpError::NotEnvelope("root is not soap:Envelope"));
    }
    Ok(envelope
        .child("Header")
        .and_then(|header| header.text_of("ID")))
}

fn parse_inform(rpc: &Node) -> Inform {
    let device_id = rpc
        .child("DeviceId")
        .map(|d| DeviceId {
            manufacturer: d.text_of("Manufacturer").unwrap_or_default(),
            oui: d.text_of("OUI").unwrap_or_default(),
            product_class: d.text_of("ProductClass").unwrap_or_default(),
            serial_number: d.text_of("SerialNumber").unwrap_or_default(),
        })
        .unwrap_or_default();

    let events = rpc
        .child("Event")
        .map(|ev| {
            ev.children_named("EventStruct")
                .map(|e| EventStruct {
                    event_code: e.text_of("EventCode").unwrap_or_default(),
                    command_key: e.text_of("CommandKey").unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    Inform {
        device_id,
        events,
        max_envelopes: parse_u32(rpc, "MaxEnvelopes"),
        current_time: rpc.text_of("CurrentTime").unwrap_or_default(),
        retry_count: parse_u32(rpc, "RetryCount"),
        parameter_list: parse_value_list(rpc.child("ParameterList")),
    }
}

fn parse_get_parameter_names_response(rpc: &Node) -> GetParameterNamesResponse {
    let parameters = rpc
        .child("ParameterList")
        .map(|list| {
            list.children_named("ParameterInfoStruct")
                .map(|p| ParameterInfo {
                    name: p.text_of("Name").unwrap_or_default(),
                    writable: p
                        .text_of("Writable")
                        .map(|s| parse_bool(&s))
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    GetParameterNamesResponse { parameters }
}

fn parse_value_list(list: Option<&Node>) -> Vec<ParameterValue> {
    let Some(list) = list else {
        return Vec::new();
    };
    list.children_named("ParameterValueStruct")
        .map(|p| {
            let value_node = p.child("Value");
            ParameterValue {
                name: p.text_of("Name").unwrap_or_default(),
                value: value_node.map(|v| v.text.clone()).unwrap_or_default(),
                xsi_type: value_node
                    .and_then(|v| v.attr("type"))
                    .unwrap_or(DEFAULT_XSI_TYPE)
                    .to_string(),
            }
        })
        .collect()
}

fn parse_string_list(list: Option<&Node>) -> Vec<String> {
    let Some(list) = list else {
        return Vec::new();
    };
    list.children_named("string")
        .map(|s| s.text.clone())
        .collect()
}

fn parse_fault(fault: &Node) -> Fault {
    let cwmp_fault = fault.child("detail").and_then(|d| d.child("Fault"));
    Fault {
        soap_fault_code: fault.text_of("faultcode").unwrap_or_default(),
        soap_fault_string: fault.text_of("faultstring").unwrap_or_default(),
        fault_code: cwmp_fault
            .and_then(|f| f.text_of("FaultCode"))
            .unwrap_or_default(),
        fault_string: cwmp_fault
            .and_then(|f| f.text_of("FaultString"))
            .unwrap_or_default(),
        parameter_faults: cwmp_fault
            .into_iter()
            .flat_map(|f| f.children_named("SetParameterValuesFault"))
            .map(|parameter| ParameterFault {
                parameter_name: parameter.text_of("ParameterName").unwrap_or_default(),
                fault_code: parameter.text_of("FaultCode").unwrap_or_default(),
                fault_string: parameter.text_of("FaultString").unwrap_or_default(),
            })
            .collect(),
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

pub fn write_envelope(msg: &CwmpMessage, id: &str) -> Result<String, CwmpError> {
    if let CwmpMessage::Empty = msg {
        return Ok(String::new());
    }
    let body = render_body(msg);
    Ok(wrap_envelope(id, &body, SOAP_ENC_NS))
}

pub fn write_legacy_inform_response(msg: &InformResponse, id: &str) -> String {
    let body = render_body(&CwmpMessage::InformResponse(msg.clone()));
    format!(
        "<?xml version=\"1.0\"?>\n\
<{env}:Envelope xmlns:{env}=\"{SOAP_ENV_NS}\" xmlns:{enc}=\"{}\" \
xmlns:{cw}=\"{CWMP_NS}\" xmlns:xsd=\"{XSD_NS}\" xmlns:xsi=\"{XSI_NS}\">\
<{env}:Header><{cw}:ID {env}:mustUnderstand=\"1\">{id}</{cw}:ID></{env}:Header>\
<{env}:Body>{body}</{env}:Body></{env}:Envelope>",
        SOAP_ENC_NS.trim_end_matches('/'),
        env = P_ENV,
        enc = P_ENC,
        cw = P_CWMP,
        id = escape_text(id),
    )
}

fn wrap_envelope(id: &str, body: &str, encoding_namespace: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<{env}:Envelope xmlns:{env}=\"{SOAP_ENV_NS}\" xmlns:{enc}=\"{encoding_namespace}\" \
xmlns:xsd=\"{XSD_NS}\" xmlns:xsi=\"{XSI_NS}\" xmlns:{cw}=\"{CWMP_NS}\">\
<{env}:Header><{cw}:ID {env}:mustUnderstand=\"1\">{id}</{cw}:ID></{env}:Header>\
<{env}:Body>{body}</{env}:Body></{env}:Envelope>",
        env = P_ENV,
        enc = P_ENC,
        cw = P_CWMP,
        id = escape_text(id),
        body = body,
    )
}

fn render_body(msg: &CwmpMessage) -> String {
    match msg {
        CwmpMessage::Empty => String::new(),
        CwmpMessage::Inform(m) => render_inform(m),
        CwmpMessage::InformResponse(m) => format!(
            "<{cw}:InformResponse><MaxEnvelopes>{}</MaxEnvelopes></{cw}:InformResponse>",
            m.max_envelopes,
            cw = P_CWMP,
        ),
        CwmpMessage::GetParameterNames(m) => format!(
            "<{cw}:GetParameterNames><ParameterPath>{}</ParameterPath>\
<NextLevel>{}</NextLevel></{cw}:GetParameterNames>",
            escape_text(&m.parameter_path),
            if m.next_level { 1 } else { 0 },
            cw = P_CWMP,
        ),
        CwmpMessage::GetParameterNamesResponse(m) => render_get_parameter_names_response(m),
        CwmpMessage::GetParameterValues(m) => format!(
            "<{cw}:GetParameterValues>{}</{cw}:GetParameterValues>",
            render_string_array("ParameterNames", &m.parameter_names),
            cw = P_CWMP,
        ),
        CwmpMessage::GetParameterValuesResponse(m) => format!(
            "<{cw}:GetParameterValuesResponse>{}</{cw}:GetParameterValuesResponse>",
            render_value_array(&m.parameters),
            cw = P_CWMP,
        ),
        CwmpMessage::SetParameterValues(m) => format!(
            "<{cw}:SetParameterValues>{}<ParameterKey>{}</ParameterKey></{cw}:SetParameterValues>",
            render_value_array(&m.parameter_list),
            escape_text(&m.parameter_key),
            cw = P_CWMP,
        ),
        CwmpMessage::SetParameterValuesResponse(m) => format!(
            "<{cw}:SetParameterValuesResponse><Status>{}</Status></{cw}:SetParameterValuesResponse>",
            m.status,
            cw = P_CWMP,
        ),
        CwmpMessage::AddObject(m) => format!(
            "<{cw}:AddObject><ObjectName>{}</ObjectName><ParameterKey>{}</ParameterKey></{cw}:AddObject>",
            escape_text(&m.object_name),
            escape_text(&m.parameter_key),
            cw = P_CWMP,
        ),
        CwmpMessage::AddObjectResponse(m) => format!(
            "<{cw}:AddObjectResponse><InstanceNumber>{}</InstanceNumber><Status>{}</Status></{cw}:AddObjectResponse>",
            m.instance_number,
            m.status,
            cw = P_CWMP,
        ),
        CwmpMessage::DeleteObject(m) => format!(
            "<{cw}:DeleteObject><ObjectName>{}</ObjectName><ParameterKey>{}</ParameterKey></{cw}:DeleteObject>",
            escape_text(&m.object_name),
            escape_text(&m.parameter_key),
            cw = P_CWMP,
        ),
        CwmpMessage::DeleteObjectResponse(m) => format!(
            "<{cw}:DeleteObjectResponse><Status>{}</Status></{cw}:DeleteObjectResponse>",
            m.status,
            cw = P_CWMP,
        ),
        CwmpMessage::GetRpcMethods(_) => {
            format!("<{cw}:GetRPCMethods></{cw}:GetRPCMethods>", cw = P_CWMP)
        }
        CwmpMessage::GetRpcMethodsResponse(m) => format!(
            "<{cw}:GetRPCMethodsResponse>{}</{cw}:GetRPCMethodsResponse>",
            render_string_array("MethodList", &m.method_list),
            cw = P_CWMP,
        ),
        CwmpMessage::Reboot(m) => format!(
            "<{cw}:Reboot><CommandKey>{}</CommandKey></{cw}:Reboot>",
            escape_text(&m.command_key),
            cw = P_CWMP,
        ),
        CwmpMessage::RebootResponse(_) => {
            format!("<{cw}:RebootResponse></{cw}:RebootResponse>", cw = P_CWMP)
        }
        CwmpMessage::Fault(m) => render_fault(m),
    }
}

fn render_inform(m: &Inform) -> String {
    let mut events = String::new();
    for e in &m.events {
        events.push_str(&format!(
            "<EventStruct><EventCode>{}</EventCode><CommandKey>{}</CommandKey></EventStruct>",
            escape_text(&e.event_code),
            escape_text(&e.command_key),
        ));
    }
    format!(
        "<{cw}:Inform><DeviceId>\
<Manufacturer>{mfr}</Manufacturer><OUI>{oui}</OUI>\
<ProductClass>{pc}</ProductClass><SerialNumber>{sn}</SerialNumber></DeviceId>\
<Event {enc}:arrayType=\"{cw}:EventStruct[{ecount}]\">{events}</Event>\
<MaxEnvelopes>{maxenv}</MaxEnvelopes><CurrentTime>{time}</CurrentTime>\
<RetryCount>{retry}</RetryCount>{params}</{cw}:Inform>",
        mfr = escape_text(&m.device_id.manufacturer),
        oui = escape_text(&m.device_id.oui),
        pc = escape_text(&m.device_id.product_class),
        sn = escape_text(&m.device_id.serial_number),
        ecount = m.events.len(),
        events = events,
        maxenv = m.max_envelopes,
        time = escape_text(&m.current_time),
        retry = m.retry_count,
        params = render_value_array(&m.parameter_list),
        cw = P_CWMP,
        enc = P_ENC,
    )
}

fn render_get_parameter_names_response(m: &GetParameterNamesResponse) -> String {
    let mut items = String::new();
    for p in &m.parameters {
        items.push_str(&format!(
            "<ParameterInfoStruct><Name>{}</Name><Writable>{}</Writable></ParameterInfoStruct>",
            escape_text(&p.name),
            if p.writable { 1 } else { 0 },
        ));
    }
    format!(
        "<{cw}:GetParameterNamesResponse>\
<ParameterList {enc}:arrayType=\"{cw}:ParameterInfoStruct[{n}]\">{items}</ParameterList>\
</{cw}:GetParameterNamesResponse>",
        n = m.parameters.len(),
        items = items,
        cw = P_CWMP,
        enc = P_ENC,
    )
}

fn render_value_array(values: &[ParameterValue]) -> String {
    let mut items = String::new();
    for v in values {
        let xsi_type = if v.xsi_type.is_empty() {
            DEFAULT_XSI_TYPE
        } else {
            &v.xsi_type
        };
        items.push_str(&format!(
            "<ParameterValueStruct><Name>{}</Name>\
<Value xsi:type=\"{ty}\">{val}</Value></ParameterValueStruct>",
            escape_text(&v.name),
            ty = escape_attr(xsi_type),
            val = escape_text(&v.value),
        ));
    }
    format!(
        "<ParameterList {enc}:arrayType=\"{cw}:ParameterValueStruct[{n}]\">{items}</ParameterList>",
        n = values.len(),
        items = items,
        enc = P_ENC,
        cw = P_CWMP,
    )
}

fn render_string_array(tag: &str, values: &[String]) -> String {
    let mut items = String::new();
    for v in values {
        items.push_str(&format!("<string>{}</string>", escape_text(v)));
    }
    format!(
        "<{tag} {enc}:arrayType=\"xsd:string[{n}]\">{items}</{tag}>",
        tag = tag,
        n = values.len(),
        items = items,
        enc = P_ENC,
    )
}

fn render_fault(m: &Fault) -> String {
    let soap_code = if m.soap_fault_code.is_empty() {
        "Client"
    } else {
        &m.soap_fault_code
    };
    let soap_string = if m.soap_fault_string.is_empty() {
        "CWMP fault"
    } else {
        &m.soap_fault_string
    };
    let parameter_faults = m
        .parameter_faults
        .iter()
        .map(|fault| {
            format!(
                "<SetParameterValuesFault><ParameterName>{}</ParameterName><FaultCode>{}</FaultCode><FaultString>{}</FaultString></SetParameterValuesFault>",
                escape_text(&fault.parameter_name),
                escape_text(&fault.fault_code),
                escape_text(&fault.fault_string)
            )
        })
        .collect::<String>();
    format!(
        "<{env}:Fault><faultcode>{code}</faultcode><faultstring>{string}</faultstring>\
<detail><{cw}:Fault><FaultCode>{fc}</FaultCode><FaultString>{fs}</FaultString>{parameter_faults}</{cw}:Fault>\
</detail></{env}:Fault>",
        code = escape_text(soap_code),
        string = escape_text(soap_string),
        fc = escape_text(&m.fault_code),
        fs = escape_text(&m.fault_string),
        parameter_faults = parameter_faults,
        env = P_ENV,
        cw = P_CWMP,
    )
}

#[cfg(test)]
mod tests;
