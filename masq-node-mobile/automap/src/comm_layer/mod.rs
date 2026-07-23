// Copyright (c) 2019-2021, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use crossbeam_channel::Sender;

use masq_lib::utils::AutomapProtocol;

use crate::comm_layer::pcp_pmp_common::MappingConfig;
use crate::control_layer::automap_control::ChangeHandler;
use crate::protocols::utils::ParseError;

pub mod igdp;
pub mod pcp;
pub mod pcp_pmp_common;
pub mod pmp;

pub const DEFAULT_MAPPING_LIFETIME_SECONDS: u32 = 600; // ten minutes

#[derive(Clone, PartialEq, Eq)]
pub enum AutomapErrorCause {
    UserError,
    NetworkConfiguration,
    ProtocolNotImplemented,
    ProtocolFailed,
    ProbeServerIssue,
    ProbeFailed,
    SocketFailure,
    RouterFailure,
    Unknown(String),
}

#[derive(Clone, PartialEq, Eq)]
pub enum AutomapError {
    Unknown,
    NoLocalIpAddress,
    CantFindDefaultGateway,
    IPv6Unsupported(Ipv6Addr),
    FindRouterError(String),
    GetPublicIpError(String),
    SocketBindingError(String, SocketAddr),
    SocketSendError(AutomapErrorCause),
    SocketReceiveError(AutomapErrorCause),
    PacketParseError(ParseError),
    ProtocolError(String),
    PermanentLeasesOnly,
    TemporaryMappingError(String),
    PermanentMappingError(String),
    ProbeServerConnectError(String),
    ProbeRequestError(AutomapErrorCause, String),
    ProbeReceiveError(String),
    DeleteMappingError(String),
    TransactionFailure(String),
    AllProtocolsFailed(Vec<(AutomapProtocol, AutomapError)>),
    HousekeeperAlreadyRunning,
    HousekeeperCrashed,
}

impl Debug for AutomapErrorCause {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AutomapErrorCause::UserError => "UserError",
            AutomapErrorCause::NetworkConfiguration => "NetworkConfiguration",
            AutomapErrorCause::ProtocolNotImplemented => "ProtocolNotImplemented",
            AutomapErrorCause::ProtocolFailed => "ProtocolFailed",
            AutomapErrorCause::ProbeServerIssue => "ProbeServerIssue",
            AutomapErrorCause::ProbeFailed => "ProbeFailed",
            AutomapErrorCause::SocketFailure => "SocketFailure",
            AutomapErrorCause::RouterFailure => "RouterFailure",
            AutomapErrorCause::Unknown(_) => "Unknown([REDACTED])",
        })
    }
}

impl Debug for AutomapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AutomapError::Unknown => f.write_str("Unknown"),
            AutomapError::NoLocalIpAddress => f.write_str("NoLocalIpAddress"),
            AutomapError::CantFindDefaultGateway => f.write_str("CantFindDefaultGateway"),
            AutomapError::IPv6Unsupported(_) => f.write_str("IPv6Unsupported([REDACTED])"),
            AutomapError::FindRouterError(_) => f.write_str("FindRouterError([REDACTED])"),
            AutomapError::GetPublicIpError(_) => f.write_str("GetPublicIpError([REDACTED])"),
            AutomapError::SocketBindingError(_, _) => f.write_str("SocketBindingError([REDACTED])"),
            AutomapError::SocketSendError(cause) => {
                write!(f, "SocketSendError({:?})", cause)
            }
            AutomapError::SocketReceiveError(cause) => {
                write!(f, "SocketReceiveError({:?})", cause)
            }
            AutomapError::PacketParseError(_) => f.write_str("PacketParseError([REDACTED])"),
            AutomapError::ProtocolError(_) => f.write_str("ProtocolError([REDACTED])"),
            AutomapError::PermanentLeasesOnly => f.write_str("PermanentLeasesOnly"),
            AutomapError::TemporaryMappingError(_) => {
                f.write_str("TemporaryMappingError([REDACTED])")
            }
            AutomapError::PermanentMappingError(_) => {
                f.write_str("PermanentMappingError([REDACTED])")
            }
            AutomapError::ProbeServerConnectError(_) => {
                f.write_str("ProbeServerConnectError([REDACTED])")
            }
            AutomapError::ProbeRequestError(cause, _) => {
                write!(f, "ProbeRequestError({:?}, [REDACTED])", cause)
            }
            AutomapError::ProbeReceiveError(_) => f.write_str("ProbeReceiveError([REDACTED])"),
            AutomapError::DeleteMappingError(_) => f.write_str("DeleteMappingError([REDACTED])"),
            AutomapError::TransactionFailure(_) => f.write_str("TransactionFailure([REDACTED])"),
            AutomapError::AllProtocolsFailed(errors) => write!(
                f,
                "AllProtocolsFailed({} protocol error(s); details redacted)",
                errors.len()
            ),
            AutomapError::HousekeeperAlreadyRunning => f.write_str("HousekeeperAlreadyRunning"),
            AutomapError::HousekeeperCrashed => f.write_str("HousekeeperCrashed"),
        }
    }
}

impl AutomapError {
    pub fn cause(&self) -> AutomapErrorCause {
        match self {
            AutomapError::Unknown => AutomapErrorCause::Unknown("Explicitly unknown".to_string()),
            AutomapError::NoLocalIpAddress => AutomapErrorCause::NetworkConfiguration,
            AutomapError::CantFindDefaultGateway => AutomapErrorCause::ProtocolNotImplemented,
            AutomapError::IPv6Unsupported(_) => AutomapErrorCause::NetworkConfiguration,
            AutomapError::FindRouterError(_) => AutomapErrorCause::NetworkConfiguration,
            AutomapError::GetPublicIpError(_) => AutomapErrorCause::ProtocolNotImplemented,
            AutomapError::SocketBindingError(_, _) => AutomapErrorCause::UserError,
            AutomapError::SocketSendError(aec) => aec.clone(),
            AutomapError::SocketReceiveError(aec) => aec.clone(),
            AutomapError::PacketParseError(_) => AutomapErrorCause::ProtocolNotImplemented,
            AutomapError::ProtocolError(_) => AutomapErrorCause::ProtocolNotImplemented,
            AutomapError::PermanentLeasesOnly => {
                AutomapErrorCause::Unknown("Can't handle permanent-only leases".to_string())
            }
            AutomapError::PermanentMappingError(_) => AutomapErrorCause::ProtocolFailed,
            AutomapError::TemporaryMappingError(_) => AutomapErrorCause::RouterFailure,
            AutomapError::ProbeServerConnectError(_) => AutomapErrorCause::ProbeServerIssue,
            AutomapError::ProbeRequestError(aec, _) => aec.clone(),
            AutomapError::ProbeReceiveError(_) => AutomapErrorCause::ProbeFailed,
            AutomapError::DeleteMappingError(_) => AutomapErrorCause::ProtocolFailed,
            AutomapError::TransactionFailure(_) => AutomapErrorCause::ProtocolFailed,
            AutomapError::AllProtocolsFailed(_) => AutomapErrorCause::NetworkConfiguration,
            AutomapError::HousekeeperAlreadyRunning => {
                AutomapErrorCause::Unknown("Sequencing error".to_string())
            }
            AutomapError::HousekeeperCrashed => {
                AutomapErrorCause::Unknown("Thread crash".to_string())
            }
        }
    }
}

pub trait Transactor {
    fn find_routers(&self) -> Result<Vec<IpAddr>, AutomapError>;
    fn get_public_ip(&self, router_ip: IpAddr) -> Result<IpAddr, AutomapError>;
    fn add_mapping(
        &self,
        router_ip: IpAddr,
        hole_port: u16,
        lifetime: u32,
    ) -> Result<u32, AutomapError>;
    fn add_permanent_mapping(&self, router_ip: IpAddr, hole_port: u16)
        -> Result<u32, AutomapError>;
    fn delete_mapping(&self, router_ip: IpAddr, hole_port: u16) -> Result<(), AutomapError>;
    fn protocol(&self) -> AutomapProtocol;
    fn start_housekeeping_thread(
        &mut self,
        change_handler: ChangeHandler,
        router_ip: IpAddr,
    ) -> Result<Sender<HousekeepingThreadCommand>, AutomapError>;
    fn stop_housekeeping_thread(&mut self) -> Result<ChangeHandler, AutomapError>;
    fn as_any(&self) -> &dyn Any;
}

impl Debug for dyn Transactor {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} Transactor", self.protocol())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HousekeepingThreadCommand {
    Stop,
    SetRemapIntervalMs(u64),
    InitializeMappingConfig(MappingConfig),
}

pub trait LocalIpFinder: Send {
    fn find(&self) -> Result<IpAddr, AutomapError>;
}

#[derive(Clone)]
pub struct LocalIpFinderReal {}

impl LocalIpFinder for LocalIpFinderReal {
    fn find(&self) -> Result<IpAddr, AutomapError> {
        match local_ipaddress::get() {
            Some(ip_str) => parse_local_ip(&ip_str),
            None => Err(AutomapError::NoLocalIpAddress),
        }
    }
}

fn parse_local_ip(ip_str: &str) -> Result<IpAddr, AutomapError> {
    IpAddr::from_str(ip_str).map_err(|_| AutomapError::NoLocalIpAddress)
}

impl Default for LocalIpFinderReal {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalIpFinderReal {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ip_parser_accepts_a_valid_address() {
        assert_eq!(
            parse_local_ip("192.0.2.1"),
            Ok(IpAddr::from_str("192.0.2.1").unwrap())
        );
    }

    #[test]
    fn local_ip_parser_rejects_malformed_library_output_without_panicking() {
        assert_eq!(
            parse_local_ip("SENSITIVE_INVALID_LOCAL_IP"),
            Err(AutomapError::NoLocalIpAddress)
        );
    }

    #[test]
    fn causes_work() {
        let errors_and_expectations = vec![
            (
                AutomapError::Unknown,
                AutomapErrorCause::Unknown("Explicitly unknown".to_string()),
            ),
            (
                AutomapError::NoLocalIpAddress,
                AutomapErrorCause::NetworkConfiguration,
            ),
            (
                AutomapError::CantFindDefaultGateway,
                AutomapErrorCause::ProtocolNotImplemented,
            ),
            (
                AutomapError::IPv6Unsupported(Ipv6Addr::from_str("::").unwrap()),
                AutomapErrorCause::NetworkConfiguration,
            ),
            (
                AutomapError::FindRouterError(String::new()),
                AutomapErrorCause::NetworkConfiguration,
            ),
            (
                AutomapError::GetPublicIpError(String::new()),
                AutomapErrorCause::ProtocolNotImplemented,
            ),
            (
                AutomapError::SocketBindingError(
                    String::new(),
                    SocketAddr::from_str("1.2.3.4:1234").unwrap(),
                ),
                AutomapErrorCause::UserError,
            ),
            (
                AutomapError::SocketSendError(AutomapErrorCause::Unknown("Booga".to_string())),
                AutomapErrorCause::Unknown("Booga".to_string()),
            ),
            (
                AutomapError::SocketReceiveError(AutomapErrorCause::Unknown("Booga".to_string())),
                AutomapErrorCause::Unknown("Booga".to_string()),
            ),
            (
                AutomapError::PacketParseError(ParseError::WrongVersion(3)),
                AutomapErrorCause::ProtocolNotImplemented,
            ),
            (
                AutomapError::ProtocolError(String::new()),
                AutomapErrorCause::ProtocolNotImplemented,
            ),
            (
                AutomapError::PermanentLeasesOnly,
                AutomapErrorCause::Unknown("Can't handle permanent-only leases".to_string()),
            ),
            (
                AutomapError::PermanentMappingError(String::new()),
                AutomapErrorCause::ProtocolFailed,
            ),
            (
                AutomapError::TemporaryMappingError(String::new()),
                AutomapErrorCause::RouterFailure,
            ),
            (
                AutomapError::ProbeServerConnectError(String::new()),
                AutomapErrorCause::ProbeServerIssue,
            ),
            (
                AutomapError::ProbeRequestError(AutomapErrorCause::ProbeFailed, String::new()),
                AutomapErrorCause::ProbeFailed,
            ),
            (
                AutomapError::ProbeReceiveError(String::new()),
                AutomapErrorCause::ProbeFailed,
            ),
            (
                AutomapError::DeleteMappingError(String::new()),
                AutomapErrorCause::ProtocolFailed,
            ),
            (
                AutomapError::TransactionFailure(String::new()),
                AutomapErrorCause::ProtocolFailed,
            ),
            (
                AutomapError::AllProtocolsFailed(vec![]),
                AutomapErrorCause::NetworkConfiguration,
            ),
            (
                AutomapError::HousekeeperAlreadyRunning,
                AutomapErrorCause::Unknown("Sequencing error".to_string()),
            ),
        ];

        let errors_and_actuals = errors_and_expectations
            .iter()
            .map(|(error, _)| (error.clone(), error.cause()))
            .collect::<Vec<(AutomapError, AutomapErrorCause)>>();

        assert_eq!(errors_and_actuals, errors_and_expectations);
    }

    #[test]
    fn automap_error_debug_redacts_addresses_and_free_form_details() {
        let sensitive = "SENSITIVE_ROUTER_OR_OS_DETAIL";
        let errors = vec![
            AutomapError::SocketBindingError(
                sensitive.to_string(),
                SocketAddr::from_str("203.0.113.241:5351").unwrap(),
            ),
            AutomapError::ProtocolError(sensitive.to_string()),
            AutomapError::AllProtocolsFailed(vec![(
                AutomapProtocol::Pcp,
                AutomapError::FindRouterError(sensitive.to_string()),
            )]),
        ];

        let rendered = format!("{:?}", errors);

        assert!(rendered.contains("SocketBindingError([REDACTED])"));
        assert!(rendered.contains("ProtocolError([REDACTED])"));
        assert!(rendered.contains("AllProtocolsFailed(1 protocol error(s); details redacted)"));
        assert!(!rendered.contains(sensitive));
        assert!(!rendered.contains("203.0.113.241"));
    }
}
