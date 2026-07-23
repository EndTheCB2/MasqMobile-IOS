// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::sub_lib::dispatcher::Endpoint;
use crate::sub_lib::neighborhood::NodeQueryResponseMetadata;
use actix::Message;
use std::fmt::{Debug, Formatter};

// This message can be sent either to a neighboring Node or to the client, but not to the server.
#[derive(PartialEq, Eq, Message, Clone)]
pub struct TransmitDataMsg {
    pub endpoint: Endpoint,
    pub last_data: bool,
    pub sequence_number_opt: Option<u64>, // Some implies clear data; None implies clandestine.
    pub data: Vec<u8>,
}

impl Debug for TransmitDataMsg {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let endpoint_kind = match &self.endpoint {
            Endpoint::Key(_) => "key",
            Endpoint::Socket(_) => "socket",
        };
        write!(
            f,
            "TransmitDataMsg {{ endpoint_kind: {}, last_data: {}, sequence_number: {:?}, data_len: {}, transmit_data: [REDACTED] }}",
            endpoint_kind,
            self.last_data,
            self.sequence_number_opt,
            self.data.len()
        )
    }
}

#[derive(Message, Clone, PartialEq, Eq)]
pub struct DispatcherNodeQueryResponse {
    pub result: Option<NodeQueryResponseMetadata>,
    pub context: TransmitDataMsg,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmit_data_debug_redacts_endpoint_and_payload() {
        let message = TransmitDataMsg {
            endpoint: Endpoint::Socket("203.0.113.71:8765".parse().unwrap()),
            last_data: false,
            sequence_number_opt: Some(7),
            data: b"private transmit marker".to_vec(),
        };

        assert_eq!(
            format!("{:?}", message),
            "TransmitDataMsg { endpoint_kind: socket, last_data: false, sequence_number: Some(7), data_len: 23, transmit_data: [REDACTED] }"
        );
    }
}
