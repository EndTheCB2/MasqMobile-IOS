use std::fmt::{Debug, Display};

#[derive(Clone, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    pub port: u16,
}

impl Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "Host {{ name: [REDACTED], port: {} }}", self.port)
    }
}

impl Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        write!(f, "{}:{}", self.name, self.port)
    }
}

impl Host {
    pub fn new(name: &str, port: u16) -> Host {
        Host {
            name: name.to_string(),
            port,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let subject = Host {
            name: "example.com".to_string(),
            port: 8080,
        };

        let result = format!("{}", subject);

        assert_eq!(result, "example.com:8080".to_string());
    }

    #[test]
    fn debug_redacts_destination_name() {
        let subject = Host::new("sensitive.destination.example", 8443);

        assert_eq!(
            format!("{:?}", subject),
            "Host { name: [REDACTED], port: 8443 }"
        );
    }
}
