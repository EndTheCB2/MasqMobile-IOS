// Copyright (c) 2019-2021, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

#![cfg(target_os = "macos")]

use crate::comm_layer::pcp_pmp_common::FindRoutersCommand;
use crate::comm_layer::AutomapError;
use masq_lib::utils::to_string;
use std::net::IpAddr;
use std::str::FromStr;

pub fn macos_find_routers(command: &dyn FindRoutersCommand) -> Result<Vec<IpAddr>, AutomapError> {
    let output = match command.execute() {
        Ok(stdout) => stdout,
        Err(stderr) => return Err(AutomapError::ProtocolError(stderr)),
    };
    output
        .split('\n')
        .map(to_string)
        .filter(|line| line.contains("gateway:"))
        .map(|line| line.split(": ").map(to_string).collect::<Vec<String>>())
        .filter(|pieces| pieces.len() > 1)
        .map(|pieces| {
            IpAddr::from_str(&pieces[1]).map_err(|_| {
                AutomapError::FindRouterError(
                    "Could not parse the default gateway from route output".to_string(),
                )
            })
        })
        .collect::<Result<Vec<IpAddr>, AutomapError>>()
}

pub struct MacOsFindRoutersCommand {}

impl FindRoutersCommand for MacOsFindRoutersCommand {
    fn execute(&self) -> Result<String, String> {
        self.execute_command("route -n get default")
    }
}

impl Default for MacOsFindRoutersCommand {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsFindRoutersCommand {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mocks::FindRoutersCommandMock;
    use std::collections::HashSet;
    use std::str::FromStr;

    #[test]
    fn find_routers_works_when_there_are_multiple_routers_to_find() {
        let route_n_output = "   route to: default
destination: default
       mask: default
    gateway: 192.168.0.1
    gateway: 192.168.0.2
  interface: en0
      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING>
 recvpipe  sendpipe  ssthresh  rtt,msec    rttvar  hopcount      mtu     expire
       0         0         0         0         0         0      1500         0
";
        let find_routers_command = FindRoutersCommandMock::new(Ok(&route_n_output));

        let result = macos_find_routers(&find_routers_command).unwrap();

        assert_eq!(
            result,
            vec![
                IpAddr::from_str("192.168.0.1").unwrap(),
                IpAddr::from_str("192.168.0.2").unwrap(),
            ]
        )
    }

    #[test]
    fn find_routers_works_when_there_is_no_router_to_find() {
        let route_n_output = "   route to: default
destination: default
       mask: default
      belch: 192.168.0.1
  interface: en0
      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING>
 recvpipe  sendpipe  ssthresh  rtt,msec    rttvar  hopcount      mtu     expire
       0         0         0         0         0         0      1500         0
";
        let find_routers_command = FindRoutersCommandMock::new(Ok(&route_n_output));

        let result = macos_find_routers(&find_routers_command).unwrap();

        assert_eq!(result.is_empty(), true)
    }

    #[test]
    fn find_routers_works_when_command_writes_to_stderr() {
        let find_routers_command = FindRoutersCommandMock::new(Err("Booga!"));

        let result = macos_find_routers(&find_routers_command);

        assert_eq!(
            result,
            Err(AutomapError::ProtocolError("Booga!".to_string()))
        )
    }

    #[test]
    fn find_routers_rejects_a_malformed_gateway_without_panicking() {
        let find_routers_command =
            FindRoutersCommandMock::new(Ok("gateway: SENSITIVE_INVALID_GATEWAY"));

        let result = macos_find_routers(&find_routers_command);

        assert_eq!(
            result,
            Err(AutomapError::FindRouterError(
                "Could not parse the default gateway from route output".to_string()
            ))
        );
    }

    #[test]
    fn find_routers_command_produces_output_that_looks_right() {
        let subject = MacOsFindRoutersCommand::new();

        let result = subject.execute().unwrap();

        let lines = result
            .split('\n')
            .flat_map(|line| {
                let columns = line.split(": ").collect::<Vec<&str>>();
                if columns.len() == 2 {
                    Some(columns[0])
                } else {
                    None
                }
            })
            .map(|header| header.trim())
            .collect::<HashSet<&str>>();
        assert_eq!(
            lines,
            vec![
                "route to",
                "destination",
                "mask",
                "gateway",
                "interface",
                "flags"
            ]
            .into_iter()
            .collect::<HashSet<&str>>()
        );
    }
}
