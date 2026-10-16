use super::*;

/// Max serialized SOAP body the ACS sends to a CPE. The Airvana tragent reads
/// the request into a fixed ~4192-byte buffer and silently drops anything larger
/// without a fault, so requests are split to stay at or under this.
const MAX_SOAP_BODY: usize = 4096;

/// Worst-case `cwmp:ID` for measuring envelope size. The real session id is never
/// longer, so a chunk measured within budget stays within budget on the wire.
const SIZE_MEASURE_ID: &str = "s000000000000000000000";

fn envelope_size(msg: &CwmpMessage) -> usize {
    cdma_cwmp::write_envelope(msg, SIZE_MEASURE_ID)
        .map(|xml| xml.len())
        .unwrap_or(usize::MAX)
}

pub(super) fn split_request(msg: CwmpMessage, batch_queries: bool) -> Vec<CwmpMessage> {
    let force_single_query = !batch_queries && matches!(msg, CwmpMessage::GetParameterValues(_));
    if !force_single_query && envelope_size(&msg) <= MAX_SOAP_BODY {
        return vec![msg];
    }
    match msg {
        CwmpMessage::GetParameterValues(request) if force_single_query => request
            .parameter_names
            .into_iter()
            .map(|name| {
                CwmpMessage::GetParameterValues(GetParameterValues {
                    parameter_names: vec![name],
                })
            })
            .collect(),
        CwmpMessage::GetParameterValues(request) => chunk_by(request.parameter_names, |names| {
            CwmpMessage::GetParameterValues(GetParameterValues {
                parameter_names: names,
            })
        }),
        CwmpMessage::SetParameterValues(request) => {
            let key = request.parameter_key;
            chunk_by(request.parameter_list, move |list| {
                CwmpMessage::SetParameterValues(SetParameterValues {
                    parameter_list: list,
                    parameter_key: key.clone(),
                })
            })
        }
        other => {
            log::warn!(
                "ACS: outgoing request serializes to {} bytes (> {MAX_SOAP_BODY}) and is not \
                 splittable; sending as-is",
                envelope_size(&other)
            );
            vec![other]
        }
    }
}

fn chunk_by<T: Clone>(items: Vec<T>, build: impl Fn(Vec<T>) -> CwmpMessage) -> Vec<CwmpMessage> {
    let mut chunks = Vec::new();
    let mut current: Vec<T> = Vec::new();
    for item in items {
        current.push(item);
        if current.len() > 1 && envelope_size(&build(current.clone())) > MAX_SOAP_BODY {
            let overflow = current.pop().expect("just pushed an item");
            chunks.push(build(std::mem::take(&mut current)));
            current.push(overflow);
        }
    }
    if !current.is_empty() {
        chunks.push(build(current));
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_get_parameter_values_splits_under_cap() {
        let names: Vec<String> = (0..60)
            .map(|i| format!(
                "InternetGatewayDevice.Services.X_0005B9_OneXMO.X_0005B9_BatchTableOneXPcfPdsn.BatchTableOneXPcfPdsnTableEntry.1.OneXbatchTableSampleParameterName{i:03}"
            ))
            .collect();
        let msg = CwmpMessage::GetParameterValues(GetParameterValues {
            parameter_names: names.clone(),
        });
        assert!(
            envelope_size(&msg) > MAX_SOAP_BODY,
            "setup: request must exceed cap"
        );
        let chunks = split_request(msg, true);
        assert!(chunks.len() > 1, "request should be split");
        let mut seen = Vec::new();
        for chunk in &chunks {
            assert!(
                envelope_size(chunk) <= MAX_SOAP_BODY,
                "chunk exceeds cap: {} bytes",
                envelope_size(chunk)
            );
            if let CwmpMessage::GetParameterValues(g) = chunk {
                seen.extend(g.parameter_names.iter().cloned());
            }
        }
        assert_eq!(
            seen, names,
            "every name preserved in order, no loss or duplication"
        );
    }
}
