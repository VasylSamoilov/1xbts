use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use cdma_common::consts::BURST_TYPE_OTASP;
use cdma_ios::EncodedA1Message;
use cdma_sip::{SipRequest, SipResponse};

use super::{
    A1_CAUSE_NORMAL_CALL_CLEARING, ClearRequestMessage, ImsCore, Message, MessageType, Method,
    SC_BAD_REQUEST, SC_NOT_IMPLEMENTED, SC_OK, SIP_INVITE_TIMEOUT, UplinkObservation,
    request_femto_message_with_fx2, response,
};

// RFC 3261 §13.3.1.4 and §17.1.1.1 define T1, T2 and the 64*T1 ACK deadline.
pub(super) const SIP_T1: Duration = Duration::from_millis(500);
const SIP_T2: Duration = Duration::from_secs(4);
const MAX_CACHED_ACK_TRANSACTIONS: usize = 4096;

pub(super) struct PendingOtaspAnswer {
    bursts: Vec<EncodedA1Message>,
    pub(super) response: SipResponse,
    peer: SocketAddr,
    retry_interval: Duration,
    next_retry: Instant,
    expires: Instant,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct AddsAckTransaction {
    peer: SocketAddr,
    branch: String,
    call_id: String,
    cseq: u32,
    method: String,
}

impl AddsAckTransaction {
    pub(super) fn from_request(request: &SipRequest, peer: SocketAddr) -> Option<Self> {
        let (cseq, method) = request.headers.cseq()?;
        if method != request.method.to_string() {
            return None;
        }
        Some(Self {
            peer,
            branch: request.headers.via_branch()?,
            call_id: request.headers.call_id()?.to_owned(),
            cseq,
            method,
        })
    }
}

#[derive(Default)]
pub(super) struct AddsAckResponses {
    transactions: HashMap<AddsAckTransaction, (Instant, SipResponse)>,
}

impl AddsAckResponses {
    pub(super) fn expire(&mut self, now: Instant) {
        self.transactions.retain(|_, (expires, _)| *expires > now);
    }

    pub(super) fn response(
        &mut self,
        key: &AddsAckTransaction,
        now: Instant,
    ) -> Option<SipResponse> {
        self.expire(now);
        self.transactions
            .get(key)
            .map(|(_, response)| response.clone())
    }

    pub(super) fn remember(
        &mut self,
        key: AddsAckTransaction,
        response: SipResponse,
        now: Instant,
    ) {
        self.expire(now);
        if self.transactions.len() >= MAX_CACHED_ACK_TRANSACTIONS
            && let Some(oldest) = self
                .transactions
                .iter()
                .min_by_key(|(_, (expires, _))| *expires)
                .map(|(key, _)| key.clone())
        {
            self.transactions.remove(&oldest);
        }
        self.transactions
            .insert(key, (now + SIP_INVITE_TIMEOUT, response));
    }
}

impl ImsCore {
    // The local OTASP coordinator has no remote called party to answer the INVITE.
    pub(super) fn stage_otasp_answer(
        &self,
        encoded: &EncodedA1Message,
    ) -> Result<Option<(SipResponse, SocketAddr)>, crate::fcs::FcsError> {
        if encoded.message_type() != MessageType::AddsDeliver {
            return Ok(None);
        }
        let deliver = cdma_ios::AddsDeliverMessage::decode(&encoded.decode()?.payload)?;
        if deliver.adds_user_part.burst_type != BURST_TYPE_OTASP {
            return Ok(None);
        }
        let Some(call_id) = encoded.call_id() else {
            return Ok(None);
        };
        let mut pending = self.pending_otasp_answer.lock().expect("OTASP answer lock");
        if let Some(answer) = pending.get_mut(&call_id) {
            answer.bursts.push(encoded.clone());
            return Ok(Some((answer.response.clone(), answer.peer)));
        }
        let Some((response, peer)) = self.mo_dialog_response(encoded, SC_OK, "OK") else {
            return Ok(None);
        };
        let now = Instant::now();
        pending.insert(
            call_id,
            PendingOtaspAnswer {
                bursts: vec![encoded.clone()],
                response: response.clone(),
                peer,
                retry_interval: SIP_T1,
                next_retry: now + SIP_T1,
                expires: now + SIP_INVITE_TIMEOUT,
            },
        );
        log::info!("FCS: OTASP call_id={call_id} waiting for SIP ACK before data bursts");
        Ok(Some((response, peer)))
    }

    pub(super) fn otasp_after_ack(
        &self,
        request: &SipRequest,
        peer: SocketAddr,
    ) -> Option<Vec<EncodedA1Message>> {
        if request.method != Method::Ack {
            return None;
        }
        let scoped = self.scoped_sip_call_id(peer, request.headers.call_id()?);
        let call_id = *self
            .mo_voice_by_sip
            .lock()
            .expect("MO voice SIP dialog lock")
            .get(&scoped)?;
        let mut pending = self.pending_otasp_answer.lock().expect("OTASP answer lock");
        let answer = pending.get(&call_id)?;
        let (invite_cseq, _) = answer.response.headers.cseq()?;
        if request.headers.cseq() != Some((invite_cseq, "ACK".to_string()))
            || answer.response.headers.from_tag().is_none()
            || answer.response.headers.to_tag().is_none()
            || request.headers.from_tag() != answer.response.headers.from_tag()
            || request.headers.to_tag() != answer.response.headers.to_tag()
            || peer != answer.peer
        {
            return None;
        }
        let answer = pending.remove(&call_id)?;
        log::info!("FCS: OTASP call_id={call_id} SIP answer acknowledged, sending data bursts");
        Some(answer.bursts)
    }

    pub(super) fn otasp_answer_timer(
        &self,
        now: Instant,
        local: SocketAddr,
    ) -> Vec<(cdma_sip::SipMessage, SocketAddr)> {
        self.adds_ack_responses
            .lock()
            .expect("ADDS ACK response lock")
            .expire(now);
        let mut outgoing = Vec::new();
        let mut expired = Vec::new();
        {
            let mut pending = self.pending_otasp_answer.lock().expect("OTASP answer lock");
            pending.retain(|call_id, answer| {
                if now >= answer.expires {
                    expired.push(*call_id);
                    return false;
                }
                if now >= answer.next_retry {
                    outgoing.push((
                        cdma_sip::SipMessage::Response(answer.response.clone()),
                        answer.peer,
                    ));
                    answer.retry_interval = (answer.retry_interval * 2).min(SIP_T2);
                    answer.next_retry = now + answer.retry_interval;
                }
                true
            });
        }
        for call_id in expired {
            log::warn!("FCS: OTASP call_id={call_id} SIP ACK timed out, clearing call");
            outgoing.extend(self.clear_dialog_requests(call_id, local));
            let clear = ClearRequestMessage {
                cause: cdma_ios::Cause(A1_CAUSE_NORMAL_CALL_CLEARING),
                cause_layer3: None,
            };
            match clear.encode() {
                Ok(payload) => {
                    self.forward_to_msc(
                        "forwarding OTASP ACK timeout as A1 Clear Request",
                        None,
                        EncodedA1Message::from_message_for_call(
                            &Message::new(MessageType::ClearRequest, payload),
                            Some(call_id),
                        ),
                        UplinkObservation::A1Wire,
                        crate::event::A1Context::default(),
                    );
                }
                Err(error) => log::warn!("FCS: encoding OTASP Clear Request: {error}"),
            }
        }
        outgoing
    }

    pub(super) fn handle_otasp_info(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        let Some(fap_id) = self.fap_id_for_peer(peer) else {
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let fx2 = self
            .fap_registry
            .get(&fap_id)
            .map(|fap| fap.implementation.fx2());
        let Some(message) = request_femto_message_with_fx2(req, fx2) else {
            return response(req, SC_NOT_IMPLEMENTED, "Not Implemented");
        };
        if message.to_adds_deliver_ack().is_err() {
            return response(req, SC_NOT_IMPLEMENTED, "Not Implemented");
        }
        self.handle_message(req, peer)
    }
}

#[cfg(test)]
mod tests;
