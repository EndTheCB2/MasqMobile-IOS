use hickory_proto::{
    op::{Message, MessageType, ResponseCode},
    rr::{
        DNSClass, RData, Record, RecordType,
        rdata::{A, AAAA},
    },
};
use std::net::IpAddr;

/// Builds a virtual-DNS response for the first question in `request`.
///
/// A synthetic address is only returned when its family matches the requested
/// record type. In particular, an IPv4 fake-IP must never be returned in
/// response to AAAA, HTTPS, SVCB, or another non-A question. Unsupported
/// questions receive a successful response with no answers (NODATA), allowing
/// clients to fall back to an A lookup without learning a bogus record.
pub fn build_dns_response(mut request: Message, ip: Option<IpAddr>, ttl: u32) -> Result<Message, String> {
    let query = request.queries.first().ok_or("DnsRequest no query body")?;
    let name = query.name().clone();
    let record = match (query.query_class(), query.query_type(), ip) {
        (DNSClass::IN, RecordType::A, Some(IpAddr::V4(ip))) => Some(Record::from_rdata(name, ttl, RData::A(A(ip)))),
        (DNSClass::IN, RecordType::AAAA, Some(IpAddr::V6(ip))) => Some(Record::from_rdata(name, ttl, RData::AAAA(AAAA(ip)))),
        _ => None,
    };

    // A DNS request should not contain response records. Clear them defensively
    // so malformed input cannot make us echo unrelated data as an answer.
    request.answers.clear();
    request.authorities.clear();
    request.additionals.clear();
    request.signature = None;
    request.metadata.message_type = MessageType::Response;
    request.metadata.response_code = ResponseCode::NoError;
    request.metadata.recursion_available = true;

    if let Some(record) = record {
        request.add_answer(record);
    }
    Ok(request)
}

pub fn remove_ipv6_entries(message: &mut Message) {
    message.answers.retain(|answer| !matches!(&answer.data, RData::AAAA(_)));
}

pub fn extract_ipaddr_from_dns_message(message: &Message) -> Result<IpAddr, String> {
    if message.metadata.response_code != ResponseCode::NoError {
        return Err(format!("{:?}", message.metadata.response_code));
    }
    let mut cname = None;
    for answer in &message.answers {
        match &answer.data {
            RData::A(addr) => {
                return Ok(IpAddr::V4((*addr).into()));
            }
            RData::AAAA(addr) => {
                return Ok(IpAddr::V6((*addr).into()));
            }
            RData::CNAME(name) => {
                cname = Some(name.to_utf8());
            }
            _ => {}
        }
    }
    if let Some(cname) = cname {
        return Err(cname);
    }
    Err(format!("{:?}", message.answers))
}

pub fn extract_domain_from_dns_message(message: &Message) -> Result<String, String> {
    let query = message.queries.first().ok_or("DnsRequest no query body")?;
    let name = query.name().to_string();
    Ok(name)
}

pub fn extract_record_type_from_dns_message(message: &Message) -> Result<RecordType, String> {
    let query = message.queries.first().ok_or("DnsRequest no query body")?;
    Ok(query.query_type())
}

pub fn extract_dns_class_from_dns_message(message: &Message) -> Result<DNSClass, String> {
    let query = message.queries.first().ok_or("DnsRequest no query body")?;
    Ok(query.query_class())
}

pub fn parse_data_to_dns_message(data: &[u8], used_by_tcp: bool) -> Result<Message, String> {
    if used_by_tcp {
        if data.len() < 2 {
            return Err("invalid dns data".into());
        }
        let len = u16::from_be_bytes([data[0], data[1]]) as usize;
        let data = data.get(2..len + 2).ok_or("invalid dns data")?;
        return parse_data_to_dns_message(data, false);
    }
    let message = Message::from_vec(data).map_err(|e| e.to_string())?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::Query;
    use hickory_proto::rr::Name;
    use std::{
        net::{Ipv4Addr, Ipv6Addr},
        str::FromStr,
    };

    fn request(record_type: RecordType) -> Message {
        let mut message = Message::query();
        message.metadata.id = 0x4d51;
        message.add_query(Query::query(Name::from_str("dns-test.invalid.").unwrap(), record_type));
        message
    }

    #[test]
    fn ipv4_fake_ip_is_only_returned_for_an_a_question() {
        let response = build_dns_response(request(RecordType::A), Some(IpAddr::V4(Ipv4Addr::new(198, 19, 20, 1))), 5).unwrap();

        assert_eq!(response.metadata.id, 0x4d51);
        assert_eq!(response.metadata.message_type, MessageType::Response);
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        assert!(response.metadata.recursion_available);
        assert_eq!(response.answers.len(), 1);
        assert!(matches!(response.answers[0].data, RData::A(_)));
        assert_eq!(response.answers[0].ttl, 5);
    }

    #[test]
    fn ipv4_fake_ip_is_not_returned_for_aaaa_https_svcb_or_other_questions() {
        for record_type in [
            RecordType::AAAA,
            RecordType::HTTPS,
            RecordType::SVCB,
            RecordType::TXT,
            RecordType::Unknown(65_000),
        ] {
            let response = build_dns_response(request(record_type), Some(IpAddr::V4(Ipv4Addr::new(198, 19, 20, 1))), 5).unwrap();

            assert_eq!(response.metadata.response_code, ResponseCode::NoError);
            assert!(response.answers.is_empty(), "unexpected answer for {record_type:?}");
        }
    }

    #[test]
    fn ipv6_fake_ip_is_only_returned_for_an_aaaa_question() {
        let response = build_dns_response(request(RecordType::AAAA), Some(IpAddr::V6(Ipv6Addr::LOCALHOST)), 5).unwrap();

        assert_eq!(response.answers.len(), 1);
        assert!(matches!(response.answers[0].data, RData::AAAA(_)));

        let response = build_dns_response(request(RecordType::A), Some(IpAddr::V6(Ipv6Addr::LOCALHOST)), 5).unwrap();
        assert!(response.answers.is_empty());
    }

    #[test]
    fn missing_synthetic_ip_produces_a_noerror_nodata_response() {
        let response = build_dns_response(request(RecordType::A), None, 5).unwrap();

        assert_eq!(response.metadata.message_type, MessageType::Response);
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        assert!(response.answers.is_empty());
    }
}
