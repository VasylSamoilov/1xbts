use cdma_events::{
    NetworkEvent,
    proto::{self, EventSource, network_event},
};
use cdma_ios::femto_interface_msg::FemtoInterfaceMsg;

pub use cdma_events::a1::{A1Context, a1_signaling_event};

pub fn femto_event(feid: &str, body: proto::femto_network_event::Body) -> NetworkEvent {
    NetworkEvent {
        source: EventSource::Femto as i32,
        body: Some(network_event::Body::Femto(proto::FemtoNetworkEvent {
            feid: feid.to_string(),
            body: Some(body),
        })),
        ..Default::default()
    }
}

pub fn provisioning_event(
    feid: &str,
    stage: proto::FapProvisioningStage,
    site_approved: bool,
    service_enabled: bool,
) -> NetworkEvent {
    femto_event(
        feid,
        proto::femto_network_event::Body::Provisioning(proto::FapProvisioningState {
            stage: stage as i32,
            site_approved,
            service_enabled,
        }),
    )
}

pub fn a1_event(feid: &str, message: &FemtoInterfaceMsg, context: &A1Context) -> NetworkEvent {
    cdma_events::a1::a1_event(EventSource::Femto, feid, message, context)
}

pub fn a1_event_from_wire(
    feid: &str,
    encoded: &cdma_ios::EncodedA1Message,
    context: &A1Context,
) -> Option<NetworkEvent> {
    cdma_events::a1::a1_event_from_wire(EventSource::Femto, feid, encoded, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioning_event_is_femto_sourced() {
        let ev = provisioning_event(
            "FEID-TEST",
            proto::FapProvisioningStage::Configured,
            true,
            false,
        );
        assert_eq!(ev.source, EventSource::Femto as i32);
        match ev.body {
            Some(network_event::Body::Femto(f)) => assert_eq!(f.feid, "FEID-TEST"),
            other => panic!("expected femto body, got {other:?}"),
        }
    }
}
