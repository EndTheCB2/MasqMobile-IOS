// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use actix::Message;
use serde_derive::{Deserialize, Serialize};
use std::fmt;

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum MessageTarget {
    ClientId(u64),
    AllExcept(u64),
    AllClients,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, Serialize, Deserialize)]
pub enum MessagePath {
    FireAndForget,
    Conversation(u64), // context_id
}

#[derive(PartialEq, Eq, Clone)]
pub struct MessageBody {
    pub opcode: String,
    pub path: MessagePath,
    pub payload: Result<String, (u64, String)>, // <success payload as JSON, (error code, error message)>
}

impl fmt::Debug for MessageBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let payload_summary = match &self.payload {
            Ok(json) => format!("payload contents redacted ({} bytes)", json.len()),
            Err((code, _)) => format!("error payload code {}; message contents redacted", code),
        };
        formatter
            .debug_struct("MessageBody")
            .field("opcode", &self.opcode)
            .field("path", &self.path)
            .field("payload", &payload_summary)
            .finish()
    }
}

#[derive(Message, PartialEq, Eq, Clone, Debug)]
pub struct NodeFromUiMessage {
    pub client_id: u64,
    pub body: MessageBody,
}

#[derive(Message, PartialEq, Eq, Clone, Debug)]
pub struct NodeToUiMessage {
    pub target: MessageTarget,
    pub body: MessageBody,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_debug_redacts_success_and_error_payload_contents() {
        let sensitive = "SENSITIVE_VALUE_SHOULD_NOT_LEAK";
        let success = NodeFromUiMessage {
            client_id: 7,
            body: MessageBody {
                opcode: "success".to_string(),
                path: MessagePath::Conversation(8),
                payload: Ok(sensitive.to_string()),
            },
        };
        let error = NodeToUiMessage {
            target: MessageTarget::ClientId(7),
            body: MessageBody {
                opcode: "error".to_string(),
                path: MessagePath::Conversation(8),
                payload: Err((4321, sensitive.to_string())),
            },
        };

        let success_debug = format!("{:?}", success);
        let error_debug = format!("{:?}", error);

        assert!(success_debug.contains(&format!(
            "payload contents redacted ({} bytes)",
            sensitive.len()
        )));
        assert!(error_debug.contains("error payload code 4321; message contents redacted"));
        assert!(!success_debug.contains(sensitive));
        assert!(!error_debug.contains(sensitive));
    }
}
