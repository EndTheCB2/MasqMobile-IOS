// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use crate::neighborhood::overall_connection_status::ConnectionStageErrors::{
    NoGossipResponseReceived, PassLoopFound, TcpConnectionFailed,
};
use crate::sub_lib::neighborhood::{
    ConnectionProgressEvent, ConnectionProgressMessage, NodeDescriptor,
};
use actix::Recipient;
use masq_lib::logger::Logger;
use masq_lib::messages::{
    ToMessageBody, UiConnectionChangeBroadcast, UiConnectionStage, UiConnectionStatusReason,
};
use masq_lib::ui_gateway::{MessageTarget, NodeToUiMessage};
use std::net::IpAddr;
use std::string::String;

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ConnectionStageErrors {
    TcpConnectionFailed,
    NoGossipResponseReceived,
    PassLoopFound,
    DebutRejected,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ConnectionStage {
    StageZero,
    TcpConnectionEstablished,
    NeighborshipEstablished,
    Failed(ConnectionStageErrors),
}

impl TryFrom<&ConnectionStage> for usize {
    type Error = ();

    fn try_from(connection_stage: &ConnectionStage) -> Result<Self, Self::Error> {
        match connection_stage {
            ConnectionStage::StageZero => Ok(0),
            ConnectionStage::TcpConnectionEstablished => Ok(1),
            ConnectionStage::NeighborshipEstablished => Ok(2),
            ConnectionStage::Failed(_) => Err(()),
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct ConnectionProgress {
    pub initial_node_descriptor: NodeDescriptor,
    pub current_peer_addr: IpAddr,
    pub connection_stage: ConnectionStage,
}

impl ConnectionProgress {
    pub fn new(node_descriptor: NodeDescriptor) -> Self {
        let peer_addr = node_descriptor
            .node_addr_opt
            .as_ref()
            .unwrap_or_else(|| {
                panic!("Unable to receive node address for initial descriptor; identity redacted")
            })
            .ip_addr();
        Self {
            initial_node_descriptor: node_descriptor,
            current_peer_addr: peer_addr,
            connection_stage: ConnectionStage::StageZero,
        }
    }

    pub fn update_stage(&mut self, logger: &Logger, connection_stage: ConnectionStage) -> bool {
        if self.connection_stage == connection_stage {
            trace!(
                logger,
                "Ignoring duplicate connection stage {:?}; Node identity redacted.",
                connection_stage
            );
            return false;
        }

        if matches!(
            (&self.connection_stage, &connection_stage),
            (
                ConnectionStage::NeighborshipEstablished,
                ConnectionStage::TcpConnectionEstablished
            )
        ) {
            trace!(
                logger,
                "Ignoring a late TCP-success event after authenticated gossip established the neighborship; Node identity redacted."
            );
            return false;
        }

        let transition_is_valid = matches!(
            (&self.connection_stage, &connection_stage),
            (
                ConnectionStage::StageZero,
                ConnectionStage::TcpConnectionEstablished
            ) | (
                ConnectionStage::StageZero,
                ConnectionStage::NeighborshipEstablished
            ) | (ConnectionStage::StageZero, ConnectionStage::Failed(_))
                | (
                    ConnectionStage::TcpConnectionEstablished,
                    ConnectionStage::NeighborshipEstablished
                )
                | (
                    ConnectionStage::TcpConnectionEstablished,
                    ConnectionStage::Failed(_)
                )
                | (
                    ConnectionStage::NeighborshipEstablished,
                    ConnectionStage::Failed(_)
                )
        );

        if !transition_is_valid {
            warning!(
                logger,
                "Ignoring out-of-order connection stage transition from {:?} to {:?}; Node identity redacted.",
                self.connection_stage,
                connection_stage
            );
            return false;
        }

        debug!(
            logger,
            "The connection stage for a Node has been updated from {:?} to {:?}; identity redacted.",
            self.connection_stage,
            connection_stage
        );

        self.connection_stage = connection_stage;
        true
    }

    pub fn handle_pass_gossip(&mut self, logger: &Logger, new_pass_target: IpAddr) -> bool {
        let preliminary_msg =
            "Pass gossip received for a new target; source and target identities redacted";
        match self.connection_stage {
            ConnectionStage::StageZero => {
                error!(
                    logger,
                    "{preliminary_msg}. Requested to update the stage from StageZero to StageZero.",
                )
            }
            ConnectionStage::TcpConnectionEstablished => {
                debug!(
                    logger,
                    "{preliminary_msg}. Updating the stage from TcpConnectionEstablished to StageZero.",
                )
            }
            _ => {
                warning!(
                    logger,
                    "{preliminary_msg}. Ignoring out-of-order pass gossip while at stage {:?}.",
                    self.connection_stage,
                );
                return false;
            }
        }

        self.connection_stage = ConnectionStage::StageZero;
        self.current_peer_addr = new_pass_target;
        true
    }

    pub fn retry_initial_node(&mut self, logger: &Logger) -> bool {
        if !matches!(self.connection_stage, ConnectionStage::Failed(_)) {
            trace!(
                logger,
                "Ignoring connection retry for a Node while at stage {:?}; identity redacted.",
                self.connection_stage
            );
            return false;
        }
        let initial_peer_addr = match self.initial_node_descriptor.node_addr_opt.as_ref() {
            Some(node_addr) => node_addr.ip_addr(),
            None => {
                warning!(
                    logger,
                    "Ignoring connection retry because the initial Node descriptor has no address."
                );
                return false;
            }
        };

        debug!(
            logger,
            "Retrying initial Node after failed connection; identities redacted."
        );
        self.current_peer_addr = initial_peer_addr;
        self.connection_stage = ConnectionStage::StageZero;
        true
    }

    pub fn reset_initial_node_for_user_retry(&mut self, logger: &Logger) {
        let Some(initial_peer_addr) = self
            .initial_node_descriptor
            .node_addr_opt
            .as_ref()
            .map(|node_addr| node_addr.ip_addr())
        else {
            warning!(
                logger,
                "Ignoring user connection retry because the initial Node descriptor has no address."
            );
            return;
        };
        debug!(
            logger,
            "Resetting initial Node handshake after an explicit user retry; identity redacted."
        );
        self.current_peer_addr = initial_peer_addr;
        self.connection_stage = ConnectionStage::StageZero;
    }

    pub fn reject_debut(&mut self, logger: &Logger) -> bool {
        if !matches!(
            self.connection_stage,
            ConnectionStage::StageZero | ConnectionStage::TcpConnectionEstablished
        ) {
            warning!(
                logger,
                "Ignoring late or duplicate Debut rejection for a Node while at stage {:?}; identity redacted.",
                self.connection_stage
            );
            return false;
        }
        self.update_stage(
            logger,
            ConnectionStage::Failed(ConnectionStageErrors::DebutRejected),
        )
    }
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum OverallConnectionStage {
    NotConnected = 0,
    ConnectedToNeighbor = 1, // When an Introduction or Standard Gossip (acceptance) is received
    RouteFound = 2, // Correlated, non-empty response data has returned over a MASQ exit route
}

impl From<OverallConnectionStage> for UiConnectionStage {
    fn from(stage: OverallConnectionStage) -> UiConnectionStage {
        match stage {
            OverallConnectionStage::NotConnected => UiConnectionStage::NotConnected,
            OverallConnectionStage::ConnectedToNeighbor => UiConnectionStage::ConnectedToNeighbor,
            OverallConnectionStage::RouteFound => UiConnectionStage::RouteFound,
        }
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct OverallConnectionStatus {
    // Transition depends on the ConnectionProgressMessage & check_connectedness(), they may not be in sync
    pub stage: OverallConnectionStage,
    // Corresponds to the initial_node_descriptors, that are entered by the user using --neighbors
    pub progress: Vec<ConnectionProgress>,
}

impl OverallConnectionStatus {
    pub fn new(initial_node_descriptors: Vec<NodeDescriptor>) -> Self {
        let progress = initial_node_descriptors
            .into_iter()
            .map(ConnectionProgress::new)
            .collect();

        Self {
            stage: OverallConnectionStage::NotConnected,
            progress,
        }
    }

    pub fn iter_initial_node_descriptors(&self) -> impl Iterator<Item = &NodeDescriptor> {
        self.progress
            .iter()
            .map(|connection_progress| &connection_progress.initial_node_descriptor)
    }

    pub fn get_connection_progress_by_ip(
        &mut self,
        peer_addr: IpAddr,
    ) -> Result<&mut ConnectionProgress, String> {
        let connection_progress_res = self
            .progress
            .iter_mut()
            .find(|connection_progress| connection_progress.current_peer_addr == peer_addr);

        match connection_progress_res {
            Some(connection_progress) => Ok(connection_progress),
            None => Err("Unable to find Node in connections; IP address redacted".to_string()),
        }
    }

    pub fn get_connection_progress_by_desc(
        &mut self,
        initial_node_descriptor: &NodeDescriptor,
    ) -> Result<&mut ConnectionProgress, String> {
        let connection_progress = self.progress.iter_mut().find(|connection_progress| {
            &connection_progress.initial_node_descriptor == initial_node_descriptor
        });

        match connection_progress {
            Some(connection_progress) => Ok(connection_progress),
            None => Err("Unable to find Node in connections; descriptor redacted".to_string()),
        }
    }

    pub fn update_connection_stage(
        connection_progress: &mut ConnectionProgress,
        event: ConnectionProgressEvent,
        logger: &Logger,
    ) -> bool {
        let mut modify_connection_progress =
            |stage: ConnectionStage| connection_progress.update_stage(logger, stage);

        match event {
            ConnectionProgressEvent::TcpConnectionSuccessful => {
                modify_connection_progress(ConnectionStage::TcpConnectionEstablished)
            }
            ConnectionProgressEvent::TcpConnectionFailed => {
                modify_connection_progress(ConnectionStage::Failed(TcpConnectionFailed))
            }
            ConnectionProgressEvent::IntroductionGossipReceived(_new_node) => {
                modify_connection_progress(ConnectionStage::NeighborshipEstablished)
            }
            ConnectionProgressEvent::StandardGossipReceived => {
                modify_connection_progress(ConnectionStage::NeighborshipEstablished)
            }
            ConnectionProgressEvent::PassGossipReceived(new_pass_target) => {
                connection_progress.handle_pass_gossip(logger, new_pass_target)
            }
            ConnectionProgressEvent::PassLoopFound => {
                modify_connection_progress(ConnectionStage::Failed(PassLoopFound))
            }
            ConnectionProgressEvent::NoGossipResponseReceived => {
                modify_connection_progress(ConnectionStage::Failed(NoGossipResponseReceived))
            }
        }
    }

    pub fn get_peer_addrs(&self) -> Vec<IpAddr> {
        self.progress
            .iter()
            .map(|connection_progress| connection_progress.current_peer_addr)
            .collect()
    }

    pub fn get_connection_progress_to_modify(
        &mut self,
        msg: &ConnectionProgressMessage,
    ) -> Result<&mut ConnectionProgress, String> {
        if let ConnectionProgressEvent::PassGossipReceived(pass_target) = msg.event {
            // Check if Pass Target can potentially create a duplicate ConnectionProgress
            let is_duplicate = self.get_peer_addrs().contains(&pass_target);

            if is_duplicate {
                return Err("Pass target is already part of different connection progress; IP address redacted".to_string());
            }
        };

        if let Ok(connection_progress) = self.get_connection_progress_by_ip(msg.peer_addr) {
            Ok(connection_progress)
        } else {
            Err("No peer found; IP address redacted".to_string())
        }
    }

    pub fn update_ocs_stage_and_send_message_to_ui(
        &mut self,
        new_stage: OverallConnectionStage,
        node_to_ui_recipient: &Recipient<NodeToUiMessage>,
        logger: &Logger,
    ) {
        let prev_stage = self.stage;
        if new_stage != prev_stage {
            self.stage = new_stage;
            crate::mobile_runtime::report_route_stage(self.stage as u8);
            OverallConnectionStatus::send_message_to_ui(self.stage.into(), node_to_ui_recipient);
            debug!(
                logger,
                "The stage of OverallConnectionStatus has been changed \
                from {:?} to {:?}. A message to the UI was also sent.",
                prev_stage,
                new_stage
            );
        } else {
            trace!(
                logger,
                "There was an attempt to update the stage of OverallConnectionStatus \
                from {:?} to {:?}. The request has been discarded.",
                prev_stage,
                new_stage
            )
        }
    }

    fn send_message_to_ui(
        stage: UiConnectionStage,
        node_to_ui_recipient: &Recipient<NodeToUiMessage>,
    ) {
        let message = NodeToUiMessage {
            target: MessageTarget::AllClients,
            body: UiConnectionChangeBroadcast { stage }.tmb(0),
        };

        node_to_ui_recipient
            .try_send(message)
            .expect("UI Gateway is unbound.");
    }

    pub fn is_empty(&self) -> bool {
        self.progress.is_empty()
    }

    pub fn remove(&mut self, index: usize) -> NodeDescriptor {
        let removed_connection_progress = self.progress.remove(index);
        removed_connection_progress.initial_node_descriptor
    }

    pub fn can_make_routes(&self) -> bool {
        self.stage() == OverallConnectionStage::RouteFound
    }

    pub fn stage(&self) -> OverallConnectionStage {
        self.stage
    }

    pub fn ui_connection_status_reason(&self) -> Option<UiConnectionStatusReason> {
        match self.stage {
            OverallConnectionStage::RouteFound => None,
            OverallConnectionStage::ConnectedToNeighbor => {
                Some(UiConnectionStatusReason::RouteNotReady)
            }
            OverallConnectionStage::NotConnected
                if !self.progress.is_empty()
                    && self.progress.iter().all(|progress| {
                        matches!(progress.connection_stage, ConnectionStage::Failed(_))
                    }) =>
            {
                Some(UiConnectionStatusReason::EntryNodesUnreachable)
            }
            OverallConnectionStage::NotConnected => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neighborhood::overall_connection_status::ConnectionStageErrors::{
        PassLoopFound, TcpConnectionFailed,
    };
    use crate::neighborhood::PublicKey;
    use crate::test_utils::neighborhood_test_utils::{make_ip, make_node, make_node_descriptor};
    use crate::test_utils::unshared_test_utils::make_node_to_ui_recipient;
    use actix::System;
    use masq_lib::blockchains::chains::Chain;
    use masq_lib::messages::{ToMessageBody, UiConnectionChangeBroadcast, UiConnectionStage};
    use masq_lib::test_utils::logging::{init_test_logging, TestLogHandler};
    use masq_lib::ui_gateway::MessageTarget;

    #[test]
    fn update_stage_tolerates_advancement() {
        let cases = vec![
            (
                ConnectionStage::StageZero,
                ConnectionStage::TcpConnectionEstablished,
            ),
            (
                ConnectionStage::TcpConnectionEstablished,
                ConnectionStage::NeighborshipEstablished,
            ),
            (
                ConnectionStage::StageZero,
                ConnectionStage::Failed(TcpConnectionFailed),
            ),
            (
                ConnectionStage::TcpConnectionEstablished,
                ConnectionStage::Failed(PassLoopFound),
            ),
            (
                ConnectionStage::NeighborshipEstablished,
                ConnectionStage::Failed(NoGossipResponseReceived),
            ),
        ];
        cases.into_iter().for_each(|(from_stage, to_stage)| {
            let mut subject = ConnectionProgress {
                initial_node_descriptor: make_node_descriptor(make_ip(1)),
                current_peer_addr: make_ip(1),
                connection_stage: from_stage,
            };

            let event_was_applied = subject.update_stage(
                &Logger::new("update_stage_tolerates_advancement"),
                to_stage.clone(),
            );

            assert!(event_was_applied);
            assert_eq!(subject.connection_stage, to_stage);
        })
    }

    #[test]
    fn update_stage_ignores_out_of_order_advancement() {
        let mut subject = ConnectionProgress {
            initial_node_descriptor: make_node_descriptor(make_ip(1)),
            current_peer_addr: make_ip(1),
            connection_stage: ConnectionStage::Failed(TcpConnectionFailed),
        };

        let event_was_applied = subject.update_stage(
            &Logger::new("update_stage_ignores_out_of_order_advancement"),
            ConnectionStage::NeighborshipEstablished,
        );

        assert!(!event_was_applied);
        assert_eq!(
            subject.connection_stage,
            ConnectionStage::Failed(TcpConnectionFailed)
        );
    }

    #[test]
    fn authenticated_gossip_can_advance_directly_from_stage_zero() {
        let mut subject = ConnectionProgress {
            initial_node_descriptor: make_node_descriptor(make_ip(1)),
            current_peer_addr: make_ip(1),
            connection_stage: ConnectionStage::StageZero,
        };

        let event_was_applied = subject.update_stage(
            &Logger::new("authenticated_gossip_can_advance_directly_from_stage_zero"),
            ConnectionStage::NeighborshipEstablished,
        );

        assert!(event_was_applied);
        assert_eq!(
            subject.connection_stage,
            ConnectionStage::NeighborshipEstablished
        );
    }

    #[test]
    fn late_tcp_success_is_idempotent_after_authenticated_gossip() {
        let mut subject = ConnectionProgress {
            initial_node_descriptor: make_node_descriptor(make_ip(1)),
            current_peer_addr: make_ip(1),
            connection_stage: ConnectionStage::NeighborshipEstablished,
        };

        let event_was_applied = subject.update_stage(
            &Logger::new("late_tcp_success_is_idempotent_after_authenticated_gossip"),
            ConnectionStage::TcpConnectionEstablished,
        );

        assert!(!event_was_applied);
        assert_eq!(
            subject.connection_stage,
            ConnectionStage::NeighborshipEstablished
        );
    }

    #[test]
    fn failed_stage_ignores_late_success_and_preserves_original_failure() {
        let mut subject = ConnectionProgress {
            initial_node_descriptor: make_node_descriptor(make_ip(1)),
            current_peer_addr: make_ip(1),
            connection_stage: ConnectionStage::Failed(TcpConnectionFailed),
        };

        let late_success_was_applied = subject.update_stage(
            &Logger::new("failed_stage_ignores_late_success_and_preserves_original_failure"),
            ConnectionStage::TcpConnectionEstablished,
        );
        let later_failure_was_applied = subject.update_stage(
            &Logger::new("failed_stage_ignores_late_success_and_preserves_original_failure"),
            ConnectionStage::Failed(PassLoopFound),
        );

        assert!(!late_success_was_applied);
        assert!(!later_failure_was_applied);
        assert_eq!(
            subject.connection_stage,
            ConnectionStage::Failed(TcpConnectionFailed)
        );
    }

    #[test]
    fn failed_connection_can_retry_its_initial_node_without_resurrecting_other_stages() {
        let initial_peer = make_ip(1);
        let passed_peer = make_ip(2);
        let initial_node_descriptor = make_node_descriptor(initial_peer);
        let mut subject = ConnectionProgress {
            initial_node_descriptor: initial_node_descriptor.clone(),
            current_peer_addr: passed_peer,
            connection_stage: ConnectionStage::Failed(NoGossipResponseReceived),
        };

        assert!(subject.retry_initial_node(&Logger::new("test")));
        assert_eq!(
            subject,
            ConnectionProgress {
                initial_node_descriptor,
                current_peer_addr: initial_peer,
                connection_stage: ConnectionStage::StageZero,
            }
        );
        assert!(!subject.retry_initial_node(&Logger::new("test")));
        assert_eq!(subject.connection_stage, ConnectionStage::StageZero);
    }

    #[test]
    fn debut_rejection_is_applied_only_to_an_in_progress_attempt() {
        let initial_node_descriptor = make_node_descriptor(make_ip(1));
        let mut subject = ConnectionProgress::new(initial_node_descriptor.clone());

        assert!(subject.reject_debut(&Logger::new("test")));
        assert_eq!(
            subject.connection_stage,
            ConnectionStage::Failed(ConnectionStageErrors::DebutRejected)
        );
        assert!(!subject.reject_debut(&Logger::new("test")));

        let mut connected_subject = ConnectionProgress {
            initial_node_descriptor,
            current_peer_addr: make_ip(1),
            connection_stage: ConnectionStage::NeighborshipEstablished,
        };
        assert!(!connected_subject.reject_debut(&Logger::new("test")));
        assert_eq!(
            connected_subject.connection_stage,
            ConnectionStage::NeighborshipEstablished
        );
    }

    #[test]
    fn update_stage_tolerates_stasis() {
        let cases = vec![
            ConnectionStage::StageZero,
            ConnectionStage::TcpConnectionEstablished,
            ConnectionStage::NeighborshipEstablished,
            ConnectionStage::Failed(TcpConnectionFailed),
        ];
        cases.into_iter().for_each(|stage| {
            let mut subject = ConnectionProgress {
                initial_node_descriptor: make_node_descriptor(make_ip(1)),
                current_peer_addr: make_ip(1),
                connection_stage: stage.clone(),
            };

            let event_was_applied =
                subject.update_stage(&Logger::new("update_stage_tolerates_stasis"), stage.clone());

            assert!(!event_was_applied);
            assert_eq!(subject.connection_stage, stage);
        })
    }

    #[test]
    #[should_panic(
        expected = "Unable to receive node address for initial descriptor; identity redacted"
    )]
    fn can_not_create_a_new_connection_without_node_addr() {
        let descriptor_with_no_ip_address = NodeDescriptor {
            blockchain: Chain::EthRopsten,
            encryption_public_key: PublicKey::from(vec![0, 0, 0]),
            node_addr_opt: None,
        };
        let _connection_progress = ConnectionProgress::new(descriptor_with_no_ip_address);
    }

    #[test]
    fn connection_progress_handles_pass_gossip_correctly_and_performs_logging_in_order() {
        init_test_logging();
        let test_name =
            "connection_progress_handles_pass_gossip_correctly_and_performs_logging_in_order";
        let ip_addr = make_ip(1);
        let initial_node_descriptor = make_node_descriptor(ip_addr);
        let mut subject = ConnectionProgress::new(initial_node_descriptor.clone());
        let pass_target = make_ip(2);
        let logger = Logger::new(test_name);
        subject.update_stage(&logger, ConnectionStage::TcpConnectionEstablished);

        subject.handle_pass_gossip(&logger, pass_target);

        assert_eq!(
            subject,
            ConnectionProgress {
                initial_node_descriptor,
                current_peer_addr: pass_target,
                connection_stage: ConnectionStage::StageZero
            }
        );
        TestLogHandler::new().assert_logs_contain_in_order(vec![
            &format!(
                "DEBUG: {test_name}: The connection stage \
                for a Node has been updated from {:?} to {:?}; identity redacted.",
                ConnectionStage::StageZero,
                ConnectionStage::TcpConnectionEstablished
            ),
            &format!("DEBUG: {test_name}: Pass gossip received for a new target; source and target identities redacted. Updating the stage from TcpConnectionEstablished to StageZero."),
        ]);
    }

    #[test]
    fn connection_progress_logs_error_while_handling_pass_gossip_in_case_tcp_connection_is_not_established(
    ) {
        init_test_logging();
        let test_name = "connection_progress_logs_error_while_handling_pass_gossip_in_case_tcp_connection_is_not_established";
        let ip_addr = make_ip(1);
        let initial_node_descriptor = make_node_descriptor(ip_addr);
        let mut subject = ConnectionProgress::new(initial_node_descriptor.clone());
        let pass_target = make_ip(2);

        subject.handle_pass_gossip(&Logger::new(test_name), pass_target);

        assert_eq!(
            subject,
            ConnectionProgress {
                initial_node_descriptor,
                current_peer_addr: pass_target,
                connection_stage: ConnectionStage::StageZero
            }
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "ERROR: {test_name}: Pass gossip received for a new target; source and target identities redacted. Requested to update the stage from StageZero to StageZero."
        ));
    }

    #[test]
    fn connection_progress_ignores_late_pass_gossip_after_neighborship_is_established() {
        let ip_addr = make_ip(1);
        let initial_node_descriptor = make_node_descriptor(ip_addr);
        let mut subject = ConnectionProgress::new(initial_node_descriptor.clone());
        subject.connection_stage = ConnectionStage::NeighborshipEstablished;
        let pass_target = make_ip(2);

        let event_was_applied = subject.handle_pass_gossip(&Logger::new("test"), pass_target);

        assert!(!event_was_applied);
        assert_eq!(
            subject,
            ConnectionProgress {
                initial_node_descriptor,
                current_peer_addr: ip_addr,
                connection_stage: ConnectionStage::NeighborshipEstablished,
            }
        );
    }

    #[test]
    fn overall_connection_stage_can_be_converted_into_usize_and_can_be_compared() {
        assert!(
            OverallConnectionStage::ConnectedToNeighbor as usize
                > OverallConnectionStage::NotConnected as usize
        );
        assert!(
            OverallConnectionStage::RouteFound as usize
                > OverallConnectionStage::ConnectedToNeighbor as usize
        );
    }

    #[test]
    fn able_to_create_overall_connection_status() {
        let node_desc_1 = make_node_descriptor(make_ip(1));
        let node_desc_2 = make_node_descriptor(make_ip(2));
        let initial_node_descriptors = vec![node_desc_1.clone(), node_desc_2.clone()];

        let subject = OverallConnectionStatus::new(initial_node_descriptors);

        assert_eq!(
            subject,
            OverallConnectionStatus {
                stage: OverallConnectionStage::NotConnected,
                progress: vec![
                    ConnectionProgress::new(node_desc_1),
                    ConnectionProgress::new(node_desc_2)
                ],
            }
        );
    }

    #[test]
    fn overall_connection_status_identifies_as_empty() {
        let subject = OverallConnectionStatus::new(vec![]);

        assert_eq!(subject.is_empty(), true);
    }

    #[test]
    fn overall_connection_status_identifies_as_non_empty() {
        let node_desc = make_node_descriptor(make_ip(1));
        let initial_node_descriptors = vec![node_desc];

        let subject = OverallConnectionStatus::new(initial_node_descriptors);

        assert_eq!(subject.is_empty(), false);
    }

    #[test]
    fn can_receive_a_result_of_connection_progress_from_peer_addr() {
        let peer_1_ip = make_ip(1);
        let peer_2_ip = make_ip(2);
        let desc_1 = make_node_descriptor(peer_1_ip);
        let desc_2 = make_node_descriptor(peer_2_ip);
        let initial_node_descriptors = vec![desc_1.clone(), desc_2.clone()];

        let mut subject = OverallConnectionStatus::new(initial_node_descriptors);

        let res_1 = subject.get_connection_progress_by_ip(peer_1_ip);
        assert_eq!(res_1, Ok(&mut ConnectionProgress::new(desc_1)));
        let res_2 = subject.get_connection_progress_by_ip(peer_2_ip);
        assert_eq!(res_2, Ok(&mut ConnectionProgress::new(desc_2)));
    }

    #[test]
    fn receives_an_error_in_receiving_connection_progress_from_unknown_ip_address() {
        let peer = make_ip(1);
        let desc = make_node_descriptor(peer);
        let initial_node_descriptors = vec![desc];
        let unknown_peer = make_ip(2);

        let mut subject = OverallConnectionStatus::new(initial_node_descriptors);

        let res = subject.get_connection_progress_by_ip(unknown_peer);
        assert_eq!(
            res,
            Err("Unable to find Node in connections; IP address redacted".to_string())
        );
    }

    #[test]
    fn can_receive_connection_progress_from_initial_node_desc() {
        let desc_1 = make_node_descriptor(make_ip(1));
        let desc_2 = make_node_descriptor(make_ip(2));
        let initial_node_descriptors = vec![desc_1.clone(), desc_2.clone()];

        let mut subject = OverallConnectionStatus::new(initial_node_descriptors);

        assert_eq!(
            subject.get_connection_progress_by_desc(&desc_1),
            Ok(&mut ConnectionProgress::new(desc_1))
        );
        assert_eq!(
            subject.get_connection_progress_by_desc(&desc_2),
            Ok(&mut ConnectionProgress::new(desc_2))
        );
    }

    #[test]
    fn can_receive_current_peer_addrs() {
        let peer_1 = make_ip(1);
        let peer_2 = make_ip(2);
        let peer_3 = make_ip(3);
        let subject = OverallConnectionStatus::new(vec![
            make_node_descriptor(peer_1),
            make_node_descriptor(peer_2),
            make_node_descriptor(peer_3),
        ]);

        let result = subject.get_peer_addrs();

        assert_eq!(result, vec![peer_1, peer_2, peer_3]);
    }

    #[test]
    fn receives_an_error_in_receiving_connection_progress_from_unknown_initial_node_desc() {
        let known_desc = make_node_descriptor(make_ip(1));
        let unknown_desc = make_node_descriptor(make_ip(2));
        let initial_node_descriptors = vec![known_desc];

        let mut subject = OverallConnectionStatus::new(initial_node_descriptors);

        assert_eq!(
            subject.get_connection_progress_by_desc(&unknown_desc),
            Err("Unable to find Node in connections; descriptor redacted".to_string())
        );
    }

    #[test]
    fn starting_descriptors_are_iterable() {
        let node_desc_1 = make_node_descriptor(make_ip(1));
        let node_desc_2 = make_node_descriptor(make_ip(2));
        let initial_node_descriptors = vec![node_desc_1.clone(), node_desc_2.clone()];
        let subject = OverallConnectionStatus::new(initial_node_descriptors);

        let mut result = subject.iter_initial_node_descriptors();

        assert_eq!(result.next(), Some(&node_desc_1));
        assert_eq!(result.next(), Some(&node_desc_2));
        assert_eq!(result.next(), None);
    }

    #[test]
    fn remove_deletes_descriptor_s_progress_and_returns_node_descriptor() {
        let node_desc_1 = make_node_descriptor(make_ip(1));
        let node_desc_2 = make_node_descriptor(make_ip(2));
        let initial_node_descriptors = vec![node_desc_1.clone(), node_desc_2.clone()];
        let mut subject = OverallConnectionStatus::new(initial_node_descriptors);

        let removed_desc = subject.remove(1);

        assert_eq!(removed_desc, node_desc_2);
    }

    #[test]
    fn updates_the_connection_stage_to_tcp_connection_established_and_performs_logging() {
        init_test_logging();
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor.clone()]);

        OverallConnectionStatus::update_connection_stage(
            subject.get_connection_progress_by_ip(node_ip_addr).unwrap(),
            ConnectionProgressEvent::TcpConnectionSuccessful,
            &Logger::new(
                "updates_the_connection_stage_to_tcp_connection_established_and_performs_logging",
            ),
        );

        assert_eq!(
            subject,
            OverallConnectionStatus {
                stage: OverallConnectionStage::NotConnected,
                progress: vec![ConnectionProgress {
                    initial_node_descriptor: node_descriptor,
                    current_peer_addr: node_ip_addr,
                    connection_stage: ConnectionStage::TcpConnectionEstablished
                }],
            }
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "DEBUG: updates_the_connection_stage_to_tcp_connection_established_and_performs_logging\
            : The connection stage for a Node has been updated from {:?} to {:?}; identity redacted.",
            ConnectionStage::StageZero,
            ConnectionStage::TcpConnectionEstablished
        ));
    }

    #[test]
    fn updates_the_connection_stage_to_failed_when_tcp_connection_fails() {
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor.clone()]);
        let connection_progress_to_modify =
            subject.get_connection_progress_by_ip(node_ip_addr).unwrap();

        OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::TcpConnectionFailed,
            &Logger::new("updates_the_connection_stage_to_failed_when_tcp_connection_fails"),
        );

        assert_eq!(
            subject,
            OverallConnectionStatus {
                stage: OverallConnectionStage::NotConnected,
                progress: vec![ConnectionProgress {
                    initial_node_descriptor: node_descriptor,
                    current_peer_addr: node_ip_addr,
                    connection_stage: ConnectionStage::Failed(TcpConnectionFailed)
                }],
            }
        )
    }

    #[test]
    fn updates_the_connection_stage_to_neighborship_established_when_introduction_gossip_is_received(
    ) {
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor.clone()]);
        let connection_progress = subject.get_connection_progress_by_ip(node_ip_addr).unwrap();
        let logger = Logger::new("updates_the_connection_stage_to_neighborship_established_when_introduction_gossip_is_received");
        OverallConnectionStatus::update_connection_stage(
            connection_progress,
            ConnectionProgressEvent::TcpConnectionSuccessful,
            &logger,
        );

        OverallConnectionStatus::update_connection_stage(
            connection_progress,
            ConnectionProgressEvent::IntroductionGossipReceived(make_ip(1)),
            &logger,
        );

        assert_eq!(
            connection_progress,
            &mut ConnectionProgress {
                initial_node_descriptor: node_descriptor,
                current_peer_addr: node_ip_addr,
                connection_stage: ConnectionStage::NeighborshipEstablished
            }
        )
    }

    #[test]
    fn updates_the_connection_stage_to_neighborship_established_when_standard_gossip_is_received() {
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor.clone()]);
        let connection_progress = subject.get_connection_progress_by_ip(node_ip_addr).unwrap();
        let logger = Logger::new("updates_the_connection_stage_to_neighborship_established_when_standard_gossip_is_received");
        OverallConnectionStatus::update_connection_stage(
            connection_progress,
            ConnectionProgressEvent::TcpConnectionSuccessful,
            &logger,
        );

        OverallConnectionStatus::update_connection_stage(
            connection_progress,
            ConnectionProgressEvent::StandardGossipReceived,
            &logger,
        );

        assert_eq!(
            connection_progress,
            &mut ConnectionProgress {
                initial_node_descriptor: node_descriptor,
                current_peer_addr: node_ip_addr,
                connection_stage: ConnectionStage::NeighborshipEstablished
            }
        )
    }

    #[test]
    fn updates_the_connection_stage_to_stage_zero_when_pass_gossip_is_received() {
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor.clone()]);
        let pass_target = make_ip(1);
        let connection_progress_to_modify =
            subject.get_connection_progress_by_ip(node_ip_addr).unwrap();
        let logger =
            Logger::new("updates_the_connection_stage_to_stage_zero_when_pass_gossip_is_received");
        OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::TcpConnectionSuccessful,
            &logger,
        );

        OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::PassGossipReceived(pass_target),
            &logger,
        );

        assert_eq!(
            subject,
            OverallConnectionStatus {
                stage: OverallConnectionStage::NotConnected,
                progress: vec![ConnectionProgress {
                    initial_node_descriptor: node_descriptor,
                    current_peer_addr: pass_target,
                    connection_stage: ConnectionStage::StageZero
                }],
            }
        )
    }

    #[test]
    fn updates_connection_stage_to_failed_when_pass_loop_is_found() {
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor.clone()]);
        let connection_progress_to_modify =
            subject.get_connection_progress_by_ip(node_ip_addr).unwrap();
        let logger = Logger::new("updates_connection_stage_to_failed_when_pass_loop_is_found");
        OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::TcpConnectionSuccessful,
            &logger,
        );

        OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::PassLoopFound,
            &logger,
        );

        assert_eq!(
            subject,
            OverallConnectionStatus {
                stage: OverallConnectionStage::NotConnected,
                progress: vec![ConnectionProgress {
                    initial_node_descriptor: node_descriptor,
                    current_peer_addr: node_ip_addr,
                    connection_stage: ConnectionStage::Failed(PassLoopFound)
                }],
            }
        )
    }

    #[test]
    fn updates_connection_stage_to_failed_when_no_gossip_response_is_received() {
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor.clone()]);
        let connection_progress_to_modify =
            subject.get_connection_progress_by_ip(node_ip_addr).unwrap();
        let logger =
            Logger::new("updates_connection_stage_to_failed_when_no_gossip_response_is_received");
        OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::TcpConnectionSuccessful,
            &logger,
        );

        OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::NoGossipResponseReceived,
            &logger,
        );

        assert_eq!(
            subject,
            OverallConnectionStatus {
                stage: OverallConnectionStage::NotConnected,
                progress: vec![ConnectionProgress {
                    initial_node_descriptor: node_descriptor,
                    current_peer_addr: node_ip_addr,
                    connection_stage: ConnectionStage::Failed(NoGossipResponseReceived)
                }],
            }
        )
    }

    #[test]
    fn connection_stage_can_be_converted_to_number() {
        assert_eq!(usize::try_from(&ConnectionStage::StageZero), Ok(0));
        assert_eq!(
            usize::try_from(&ConnectionStage::TcpConnectionEstablished),
            Ok(1)
        );
        assert_eq!(
            usize::try_from(&ConnectionStage::NeighborshipEstablished),
            Ok(2)
        );
        assert_eq!(
            usize::try_from(&ConnectionStage::Failed(TcpConnectionFailed)),
            Err(())
        );
    }

    #[test]
    fn authenticated_introduction_can_arrive_before_the_tcp_progress_event() {
        let (node_ip_addr, node_descriptor) = make_node(1);
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor]);
        let connection_progress_to_modify =
            subject.get_connection_progress_by_ip(node_ip_addr).unwrap();

        let event_was_applied = OverallConnectionStatus::update_connection_stage(
            connection_progress_to_modify,
            ConnectionProgressEvent::IntroductionGossipReceived(make_ip(1)),
            &Logger::new("authenticated_introduction_can_arrive_before_the_tcp_progress_event"),
        );

        assert!(event_was_applied);
        assert_eq!(
            connection_progress_to_modify.connection_stage,
            ConnectionStage::NeighborshipEstablished
        );
    }

    #[test]
    fn converts_connected_to_neighbor_stage_into_ui_connection_change_stage() {
        let connected_to_neighbor = OverallConnectionStage::ConnectedToNeighbor;

        let connected_to_neighbor_converted: UiConnectionStage = connected_to_neighbor.into();

        assert_eq!(
            connected_to_neighbor_converted,
            UiConnectionStage::ConnectedToNeighbor
        );
    }

    #[test]
    fn converts_three_hops_route_found_stage_into_ui_connection_change_stage() {
        let route_found = OverallConnectionStage::RouteFound;

        let route_found_converted: UiConnectionStage = route_found.into();

        assert_eq!(route_found_converted, UiConnectionStage::RouteFound);
    }

    #[test]
    fn converts_not_connected_into_ui_connection_change_stage() {
        let not_connected = OverallConnectionStage::NotConnected;

        let not_connected_converted: UiConnectionStage = not_connected.into();

        assert_eq!(not_connected_converted, UiConnectionStage::NotConnected);
    }

    #[test]
    fn connection_status_reason_is_bounded_and_derived_from_current_progress() {
        let node_descriptor = make_node_descriptor(make_ip(1));
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor]);

        assert_eq!(subject.ui_connection_status_reason(), None);
        subject.progress[0].connection_stage = ConnectionStage::Failed(TcpConnectionFailed);
        assert_eq!(
            subject.ui_connection_status_reason(),
            Some(UiConnectionStatusReason::EntryNodesUnreachable)
        );
        subject.stage = OverallConnectionStage::ConnectedToNeighbor;
        assert_eq!(
            subject.ui_connection_status_reason(),
            Some(UiConnectionStatusReason::RouteNotReady)
        );
        subject.stage = OverallConnectionStage::RouteFound;
        assert_eq!(subject.ui_connection_status_reason(), None);
    }

    #[test]
    fn we_can_ask_about_can_make_routes() {
        let node_descriptor = make_node_descriptor(make_ip(1));
        let mut subject = OverallConnectionStatus::new(vec![node_descriptor]);

        let initial_flag = subject.can_make_routes();
        subject.stage = OverallConnectionStage::RouteFound;
        let final_flag = subject.can_make_routes();

        assert_eq!(initial_flag, false);
        assert_eq!(final_flag, true);
    }

    #[test]
    fn updates_the_ocs_stage_to_three_hops_route_found() {
        init_test_logging();
        let test_name = "updates_the_ocs_stage_to_three_hops_route_found";
        let initial_stage = OverallConnectionStage::NotConnected;
        let new_stage = OverallConnectionStage::RouteFound;

        let (stage, message_opt) =
            assert_stage_and_node_to_ui_message(initial_stage, new_stage, test_name);

        assert_eq!(stage, new_stage);
        assert_eq!(
            message_opt,
            Some(NodeToUiMessage {
                target: MessageTarget::AllClients,
                body: UiConnectionChangeBroadcast {
                    stage: new_stage.into()
                }
                .tmb(0)
            })
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "DEBUG: {}: The stage of OverallConnectionStatus has been changed \
                from {:?} to {:?}. A message to the UI was also sent.",
            test_name, initial_stage, new_stage,
        ));
    }

    #[test]
    fn updates_the_ocs_stage_to_connected_to_neighbor() {
        init_test_logging();
        let test_name = "updates_the_ocs_stage_to_connected_to_neighbor";
        let initial_stage = OverallConnectionStage::NotConnected;
        let new_stage = OverallConnectionStage::ConnectedToNeighbor;

        let (stage, message_opt) =
            assert_stage_and_node_to_ui_message(initial_stage, new_stage, test_name);

        assert_eq!(stage, new_stage);
        assert_eq!(
            message_opt,
            Some(NodeToUiMessage {
                target: MessageTarget::AllClients,
                body: UiConnectionChangeBroadcast {
                    stage: new_stage.into()
                }
                .tmb(0)
            })
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "DEBUG: {}: The stage of OverallConnectionStatus has been changed \
                from {:?} to {:?}. A message to the UI was also sent.",
            test_name, initial_stage, new_stage
        ));
    }

    #[test]
    fn does_not_send_message_to_the_ui_in_case_the_stage_has_not_updated() {
        init_test_logging();
        let test_name = "does_not_send_message_to_the_ui_in_case_the_stage_has_not_updated";
        let initial_stage = OverallConnectionStage::ConnectedToNeighbor;
        let new_stage = initial_stage;

        let (stage, message_opt) =
            assert_stage_and_node_to_ui_message(initial_stage, new_stage, test_name);

        assert_eq!(stage, initial_stage);
        assert_eq!(message_opt, None);
        TestLogHandler::new().exists_log_containing(&format!(
            "TRACE: {}: There was an attempt to update the stage of OverallConnectionStatus \
            from {:?} to {:?}. The request has been discarded.",
            test_name, initial_stage, new_stage
        ));
    }

    #[test]
    fn sends_a_message_to_ui_in_case_connection_drops_from_three_hops_to_connected_to_neighbor() {
        init_test_logging();
        let test_name = "sends_a_message_to_ui_in_case_connection_drops_from_three_hops_to_connected_to_neighbor";
        let initial_stage = OverallConnectionStage::RouteFound;
        let new_stage = OverallConnectionStage::ConnectedToNeighbor;

        let (stage, message_opt) =
            assert_stage_and_node_to_ui_message(initial_stage, new_stage, test_name);

        assert_eq!(stage, new_stage);
        assert_eq!(
            message_opt,
            Some(NodeToUiMessage {
                target: MessageTarget::AllClients,
                body: UiConnectionChangeBroadcast {
                    stage: new_stage.into()
                }
                .tmb(0)
            })
        );
        TestLogHandler::new().exists_log_containing(&format!(
            "DEBUG: {}: The stage of OverallConnectionStatus has been changed \
                from {:?} to {:?}. A message to the UI was also sent.",
            test_name, initial_stage, new_stage
        ));
    }

    #[test]
    fn getter_fn_for_the_stage_of_overall_connection_status_exists() {
        let subject = OverallConnectionStatus::new(vec![make_node_descriptor(make_ip(1))]);
        assert_eq!(subject.stage(), OverallConnectionStage::NotConnected);
    }

    fn assert_stage_and_node_to_ui_message(
        initial_stage: OverallConnectionStage,
        new_stage: OverallConnectionStage,
        test_name: &str,
    ) -> (OverallConnectionStage, Option<NodeToUiMessage>) {
        let mut subject =
            OverallConnectionStatus::new(vec![make_node_descriptor(make_ip(u8::MAX))]);
        let (node_to_ui_recipient, node_to_ui_recording_arc) = make_node_to_ui_recipient();
        subject.stage = initial_stage;
        let system = System::new(test_name);

        subject.update_ocs_stage_and_send_message_to_ui(
            new_stage,
            &node_to_ui_recipient,
            &Logger::new(test_name),
        );

        System::current().stop();
        assert_eq!(system.run(), 0);
        let stage = subject.stage;
        let recording = node_to_ui_recording_arc.lock().unwrap();
        let message_opt = recording.get_record_opt::<NodeToUiMessage>(0).cloned();

        (stage, message_opt)
    }
}
