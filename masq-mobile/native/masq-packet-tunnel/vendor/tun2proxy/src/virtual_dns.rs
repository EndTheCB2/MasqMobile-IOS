use crate::error::Result;
use hashlink::{LruCache, linked_hash_map::RawEntryMut};
use std::{
    collections::HashMap,
    convert::TryInto,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};
use tproxy_config::IpCidr;

const MAPPING_TIMEOUT: u64 = 60; // Mapping timeout in seconds
const RESPONSE_TTL: u32 = 5;
const MAX_MAPPINGS_PER_POOL: usize = 4096;
const MAX_PERSISTED_POOLS: usize = 8;

struct NameCacheEntry {
    name: String,
    expiry: Instant,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PoolKey {
    network_addr: IpAddr,
    broadcast_addr: IpAddr,
}

struct VirtualDnsState {
    trailing_dot: bool,
    lru_cache: LruCache<IpAddr, NameCacheEntry>,
    name_to_ip: HashMap<String, IpAddr>,
    network_addr: IpAddr,
    broadcast_addr: IpAddr,
    next_addr: IpAddr,
}

impl VirtualDnsState {
    fn new(key: PoolKey) -> Self {
        Self {
            trailing_dot: false,
            next_addr: key.network_addr,
            name_to_ip: HashMap::default(),
            network_addr: key.network_addr,
            broadcast_addr: key.broadcast_addr,
            lru_cache: LruCache::new(MAX_MAPPINGS_PER_POOL),
        }
    }

    fn prune_expired(&mut self, now: Instant) {
        loop {
            let expired = match self.lru_cache.iter().next() {
                Some((ip, entry)) if now > entry.expiry => Some((*ip, entry.name.clone())),
                _ => None,
            };
            let Some((ip, name)) = expired else {
                break;
            };
            self.lru_cache.remove(&ip);
            self.name_to_ip.remove(&name);
        }
    }
}

static SHARED_POOLS: OnceLock<Mutex<HashMap<PoolKey, Arc<Mutex<VirtualDnsState>>>>> = OnceLock::new();

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn shared_state(ip_pool: IpCidr) -> Arc<Mutex<VirtualDnsState>> {
    let key = PoolKey {
        network_addr: ip_pool.first_address(),
        broadcast_addr: ip_pool.last_address(),
    };
    let registry = SHARED_POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = lock_unpoisoned(registry);

    if let Some(state) = registry.get(&key) {
        let state = Arc::clone(state);
        lock_unpoisoned(&state).prune_expired(Instant::now());
        return state;
    }

    // Retain active states and states with a still-valid DNS mapping. Dropped,
    // expired pools are reclaimed before enforcing the process-wide bound.
    let now = Instant::now();
    registry.retain(|_, state| {
        if Arc::strong_count(state) > 1 {
            return true;
        }
        let mut state = lock_unpoisoned(state);
        state.prune_expired(now);
        !state.lru_cache.is_empty()
    });

    let state = Arc::new(Mutex::new(VirtualDnsState::new(key)));
    if registry.len() < MAX_PERSISTED_POOLS {
        registry.insert(key, Arc::clone(&state));
    }
    state
}

/// A virtual DNS server which allocates IP addresses to clients.
/// The IP addresses are in the range of private IP addresses.
/// The DNS server is implemented as a LRU cache.
pub struct VirtualDns {
    state: Arc<Mutex<VirtualDnsState>>,
}

impl VirtualDns {
    pub fn new(ip_pool: IpCidr) -> Self {
        Self {
            state: shared_state(ip_pool),
        }
    }

    /// Returns the DNS response to send back to the client.
    pub fn generate_query(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        use crate::dns;
        let message = dns::parse_data_to_dns_message(data, false)?;
        let qname = dns::extract_domain_from_dns_message(&message)?;
        let record_type = dns::extract_record_type_from_dns_message(&message)?;
        let dns_class = dns::extract_dns_class_from_dns_message(&message)?;
        let pool_is_ipv4 = lock_unpoisoned(&self.state).network_addr.is_ipv4();
        let ip = match (dns_class, record_type, pool_is_ipv4) {
            (hickory_proto::rr::DNSClass::IN, hickory_proto::rr::RecordType::A, true)
            | (hickory_proto::rr::DNSClass::IN, hickory_proto::rr::RecordType::AAAA, false) => Some(self.find_or_allocate_ip(qname)?),
            _ => None,
        };
        let message = dns::build_dns_response(message, ip, RESPONSE_TTL)?;
        Ok(message.to_vec()?)
    }

    fn increment_ip(addr: IpAddr) -> Result<IpAddr> {
        let mut ip_bytes = match addr as IpAddr {
            IpAddr::V4(ip) => Vec::<u8>::from(ip.octets()),
            IpAddr::V6(ip) => Vec::<u8>::from(ip.octets()),
        };

        // Traverse bytes from right to left and stop when we can add one.
        for j in 0..ip_bytes.len() {
            let i = ip_bytes.len() - 1 - j;
            if ip_bytes[i] != 255 {
                // We can add 1 without carry and are done.
                ip_bytes[i] += 1;
                break;
            } else {
                // Zero this byte and carry over to the next one.
                ip_bytes[i] = 0;
            }
        }
        let addr = if addr.is_ipv4() {
            let bytes: [u8; 4] = ip_bytes.as_slice().try_into()?;
            IpAddr::V4(Ipv4Addr::from(bytes))
        } else {
            let bytes: [u8; 16] = ip_bytes.as_slice().try_into()?;
            IpAddr::V6(Ipv6Addr::from(bytes))
        };
        Ok(addr)
    }

    // This is to be called whenever we receive or send a packet on the socket
    // which connects the tun interface to the client, so existing IP address to name
    // mappings to not expire as long as the connection is active.
    pub fn touch_ip(&mut self, addr: &IpAddr) {
        let mut state = lock_unpoisoned(&self.state);
        let now = Instant::now();
        state.prune_expired(now);
        _ = state.lru_cache.get_mut(addr).map(|entry| {
            entry.expiry = Instant::now() + Duration::from_secs(MAPPING_TIMEOUT);
        });
    }

    pub fn resolve_ip(&mut self, addr: &IpAddr) -> Option<String> {
        let mut state = lock_unpoisoned(&self.state);
        state.prune_expired(Instant::now());
        state.lru_cache.get(addr).map(|entry| entry.name.clone())
    }

    fn find_or_allocate_ip(&mut self, name: String) -> Result<IpAddr> {
        let mut state = lock_unpoisoned(&self.state);
        // This function is a search and creation function.
        // Thus, it is sufficient to canonicalize the name here.
        let insert_name = if name.ends_with('.') && !state.trailing_dot {
            String::from(name.trim_end_matches('.'))
        } else {
            name
        };

        let now = Instant::now();

        state.prune_expired(now);

        // Return the IP if it is stored inside our LRU cache.
        if let Some(ip) = state.name_to_ip.get(&insert_name) {
            let ip = *ip;
            if let Some(entry) = state.lru_cache.get_mut(&ip) {
                entry.expiry = now + Duration::from_secs(MAPPING_TIMEOUT);
            }
            return Ok(ip);
        }

        if state.lru_cache.len() >= MAX_MAPPINGS_PER_POOL {
            return Err("Virtual DNS mapping capacity reached".into());
        }

        // Otherwise, store name and IP pair inside the LRU cache.
        let started_at = state.next_addr;

        loop {
            let next_addr = state.next_addr;
            if let RawEntryMut::Vacant(vacant) = state.lru_cache.raw_entry_mut().from_key(&next_addr) {
                let expiry = Instant::now() + Duration::from_secs(MAPPING_TIMEOUT);
                let name0 = insert_name.clone();
                vacant.insert(next_addr, NameCacheEntry { name: insert_name, expiry });
                state.name_to_ip.insert(name0, next_addr);
                return Ok(next_addr);
            }
            state.next_addr = Self::increment_ip(state.next_addr)?;
            if state.next_addr == state.broadcast_addr {
                // Wrap around.
                state.next_addr = state.network_addr;
            }
            if state.next_addr == started_at {
                return Err("Virtual IP space for DNS exhausted".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns;
    use hickory_proto::{
        op::{Message, MessageType, Query, ResponseCode},
        rr::{DNSClass, Name, RData, RecordType},
    };
    use std::str::FromStr;

    fn request(name: &str, record_type: RecordType) -> Vec<u8> {
        request_with_class(name, record_type, DNSClass::IN)
    }

    fn request_with_class(name: &str, record_type: RecordType, dns_class: DNSClass) -> Vec<u8> {
        let mut message = Message::query();
        message.metadata.id = 0x4453;
        let mut query = Query::query(Name::from_str(name).unwrap(), record_type);
        query.set_query_class(dns_class);
        message.add_query(query);
        message.to_vec().unwrap()
    }

    fn ipv4_pool(cidr: &str) -> IpCidr {
        IpCidr::from_str(cidr).unwrap()
    }

    #[test]
    fn unsupported_ipv4_pool_questions_return_nodata_without_allocating_mappings() {
        let mut virtual_dns = VirtualDns::new(ipv4_pool("198.19.40.0/24"));

        for (index, record_type) in [RecordType::AAAA, RecordType::HTTPS, RecordType::SVCB, RecordType::TXT]
            .into_iter()
            .enumerate()
        {
            let bytes = virtual_dns
                .generate_query(&request(&format!("type-{index}.invalid."), record_type))
                .unwrap();
            let response = dns::parse_data_to_dns_message(&bytes, false).unwrap();
            assert_eq!(response.metadata.message_type, MessageType::Response);
            assert_eq!(response.metadata.response_code, ResponseCode::NoError);
            assert!(response.answers.is_empty());
        }

        let bytes = virtual_dns
            .generate_query(&request_with_class("chaos.invalid.", RecordType::A, DNSClass::CH))
            .unwrap();
        let response = dns::parse_data_to_dns_message(&bytes, false).unwrap();
        assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        assert!(response.answers.is_empty());

        assert_eq!(lock_unpoisoned(&virtual_dns.state).lru_cache.len(), 0);
    }

    #[test]
    fn a_query_allocates_an_ipv4_mapping_and_returns_it() {
        let mut virtual_dns = VirtualDns::new(ipv4_pool("198.19.41.0/24"));
        let bytes = virtual_dns.generate_query(&request("address.invalid.", RecordType::A)).unwrap();
        let response = dns::parse_data_to_dns_message(&bytes, false).unwrap();

        assert_eq!(response.answers.len(), 1);
        assert!(matches!(response.answers[0].data, RData::A(_)));
        assert_eq!(lock_unpoisoned(&virtual_dns.state).lru_cache.len(), 1);
    }

    #[test]
    fn mapping_survives_a_translator_rebind_for_at_least_the_advertised_ttl() {
        let pool = ipv4_pool("198.19.42.0/24");
        let mut first = VirtualDns::new(pool);
        let bytes = first.generate_query(&request("rebind.invalid.", RecordType::A)).unwrap();
        let response = dns::parse_data_to_dns_message(&bytes, false).unwrap();
        let fake_ip = dns::extract_ipaddr_from_dns_message(&response).unwrap();
        let advertised_ttl = response.answers[0].ttl;

        assert!(MAPPING_TIMEOUT >= u64::from(advertised_ttl));
        drop(first);

        let mut rebound = VirtualDns::new(pool);
        assert_eq!(rebound.resolve_ip(&fake_ip).as_deref(), Some("rebind.invalid"));
    }
}
