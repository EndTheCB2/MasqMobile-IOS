// Copyright (c) 2019, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.
use crate::sub_lib::cryptde::CryptDE;
use crate::sub_lib::cryptde::CryptData;
use crate::sub_lib::cryptde::PublicKey;
use crate::sub_lib::cryptde::{decodex, CodexError};
use crate::sub_lib::dispatcher::Component;
use crate::sub_lib::hop::LiveHop;
use crate::sub_lib::service_receipt::ReceiptSessionRequest;
use crate::sub_lib::wallet::Wallet;
use ethereum_types::Address;
use itertools::Itertools;
use serde_derive::{Deserialize, Serialize};
use std::cmp::min;
use std::fmt::Debug;
use std::iter;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Route {
    pub hops: Vec<CryptData>,
}

impl Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Route {{ hop_count: {}, encrypted_hops: [REDACTED] }}",
            self.hops.len()
        )
    }
}

impl Route {
    pub fn single_hop(
        destination: &PublicKey,
        cryptde: &dyn CryptDE, // The CryptDE of the beginning of this Route must go here.
    ) -> Result<Route, CodexError> {
        Self::construct(
            RouteSegment::new(
                vec![cryptde.public_key(), destination],
                Component::Neighborhood,
            ),
            None,
            cryptde,
            None,
            None,
            None,
        )
    }

    pub fn one_way(
        route_segment: RouteSegment,
        cryptde: &dyn CryptDE, // Any CryptDE can go here; it's only used to encrypt to public keys.
        consuming_wallet: Option<Wallet>,
        contract_address: Option<Address>,
    ) -> Result<Route, CodexError> {
        Self::construct(
            route_segment,
            None,
            cryptde,
            consuming_wallet,
            contract_address,
            None,
        )
    }

    pub fn round_trip(
        route_segment_over: RouteSegment,
        route_segment_back: RouteSegment,
        cryptde: &dyn CryptDE, // Doesn't matter which CryptDE: only used for encoding.
        consuming_wallet: Option<Wallet>,
        contract_address: Option<Address>,
    ) -> Result<Route, CodexError> {
        Self::construct(
            route_segment_over,
            Some(route_segment_back),
            cryptde,
            consuming_wallet,
            contract_address,
            None,
        )
    }

    pub fn round_trip_with_receipts(
        route_segment_over: RouteSegment,
        route_segment_back: RouteSegment,
        cryptde: &dyn CryptDE,
        consuming_wallet: Option<Wallet>,
        contract_address: Option<Address>,
        routing_receipt_request_opt: Option<ReceiptSessionRequest>,
    ) -> Result<Route, CodexError> {
        Self::construct(
            route_segment_over,
            Some(route_segment_back),
            cryptde,
            consuming_wallet,
            contract_address,
            routing_receipt_request_opt,
        )
    }

    // This cryptde must be the CryptDE of the next hop to come off the Route.
    pub fn next_hop(&self, cryptde: &dyn CryptDE) -> Result<LiveHop, CodexError> {
        match self.hops.first() {
            None => Err(CodexError::RoutingError(RouteError::EmptyRoute)),
            Some(first) => LiveHop::decode(cryptde, &first.clone()),
        }
    }

    pub fn shift(&mut self, cryptde: &dyn CryptDE) -> Result<LiveHop, CodexError> {
        if self.hops.is_empty() {
            return Err(CodexError::RoutingError(RouteError::EmptyRoute));
        }
        let top_hop = self.hops.remove(0);
        let top_hop_len = top_hop.len();
        let next_hop = LiveHop::decode(cryptde, &top_hop)?;

        let mut garbage_can: Vec<u8> = iter::repeat(0u8).take(top_hop_len).collect();
        cryptde.random(&mut garbage_can[..]);
        self.hops.push(CryptData::new(&garbage_can[..]));

        Ok(next_hop)
    }

    pub fn to_string(&self, cryptdes: Vec<&dyn CryptDE>) -> String {
        let item_count = min(cryptdes.len(), self.hops.len());
        if item_count == 0 {
            return String::from("\n");
        }
        let mut most_hops_enc: Vec<CryptData> = self.hops[0..item_count].to_vec();
        let mut most_cryptdes: Vec<&dyn CryptDE> = cryptdes[0..item_count].to_vec();
        let last_hop_enc = most_hops_enc.remove(item_count - 1);
        let last_cryptde = most_cryptdes.remove(item_count - 1);
        let most_strings = (0..(item_count - 1)).fold(String::new(), |sofar, index| {
            let hop_enc = &most_hops_enc[index];
            let cryptde = most_cryptdes[index];
            let live_hop_str = match decodex::<LiveHop>(cryptde, hop_enc) {
                Ok(live_hop) => {
                    format!("Encrypted hop: {:?}", live_hop)
                }
                Err(e) => format!("Error: {}", codex_error_category(&e)),
            };
            format!("{}\n{}", sofar, live_hop_str)
        });
        match decodex::<LiveHop>(last_cryptde, &last_hop_enc) {
            Ok(live_hop) => format!("{}\nEncrypted hop: {:?}\n", most_strings, live_hop),
            Err(error) => format!("{}\nError: {}", most_strings, codex_error_category(&error)),
        }
    }

    fn construct(
        over: RouteSegment,
        back: Option<RouteSegment>,
        cryptde: &dyn CryptDE,
        consuming_wallet: Option<Wallet>,
        contract_address: Option<Address>,
        routing_receipt_request_opt: Option<ReceiptSessionRequest>,
    ) -> Result<Route, CodexError> {
        if let Some(error) = Route::validate_route_segments(&over, &back) {
            return Err(CodexError::RoutingError(error));
        }
        let over_component = over.recipient;
        let over_keys = over.keys.iter();

        let mut hops = Route::over_segment(
            back.is_none(),
            consuming_wallet.clone(),
            over_keys,
            over_component,
            contract_address,
            routing_receipt_request_opt.clone(),
        );

        Route::back_segment(
            &back,
            consuming_wallet,
            over_component,
            &mut hops,
            contract_address,
            routing_receipt_request_opt,
        );

        Route::hops_to_route(hops[0..].to_vec(), &over.keys[0], cryptde)
    }

    fn over_segment<'a>(
        one_way: bool,
        consuming_wallet_opt: Option<Wallet>,
        over_keys: impl Iterator<Item = &'a PublicKey>,
        over_component: Component,
        contract_address_opt: Option<Address>,
        routing_receipt_request_opt: Option<ReceiptSessionRequest>,
    ) -> Vec<LiveHop> {
        let mut last_key: Option<PublicKey> = None;
        let mut hops: Vec<LiveHop> = over_keys
            .tuple_windows()
            .map(|(current_key, next_key)| {
                last_key = Some(next_key.clone());
                LiveHop::new(
                    next_key,
                    consuming_wallet_opt.as_ref().map(|w| {
                        w.as_payer(
                            &current_key,
                            &contract_address_opt.unwrap_or_else(Address::zero),
                        )
                    }),
                    Component::Hopper,
                )
                .with_routing_receipt_request(routing_receipt_request_opt.clone())
            })
            .collect();
        if one_way {
            let key = PublicKey::new(b"");
            match last_key {
                Some(last_hop_key) => {
                    hops.push(
                        LiveHop::new(
                            &key,
                            consuming_wallet_opt.map(|w| {
                                w.as_payer(
                                    &last_hop_key,
                                    &contract_address_opt.unwrap_or_else(Address::zero),
                                )
                            }),
                            over_component,
                        )
                        .with_routing_receipt_request(routing_receipt_request_opt.clone()),
                    );
                }
                None => hops.push(LiveHop::new(&key, None, over_component)),
            }
        };
        hops
    }

    fn back_segment(
        back_option: &Option<RouteSegment>,
        consuming_wallet: Option<Wallet>,
        over_component: Component,
        hops: &mut Vec<LiveHop>,
        contract_address: Option<Address>,
        routing_receipt_request_opt: Option<ReceiptSessionRequest>,
    ) {
        if let Some(back) = back_option {
            let back_component = back.recipient;
            let back_keys: Vec<&PublicKey> = back.keys.iter().collect();
            for (key_index, (current_key, next_key)) in back_keys.iter().tuple_windows().enumerate()
            {
                let component = if key_index == 0 {
                    over_component
                } else {
                    Component::Hopper
                };

                hops.push(
                    LiveHop::new(
                        next_key,
                        consuming_wallet.clone().map(|w| {
                            w.as_payer(
                                &current_key,
                                &contract_address.unwrap_or_else(Address::zero),
                            )
                        }),
                        component,
                    )
                    .with_routing_receipt_request(routing_receipt_request_opt.clone()),
                )
            }
            let next_key = PublicKey::new(b"");
            match back_keys.last() {
                Some(current_key) => {
                    hops.push(
                        LiveHop::new(
                            &next_key,
                            consuming_wallet.map(|w| {
                                w.as_payer(
                                    current_key,
                                    &contract_address.unwrap_or_else(Address::zero),
                                )
                            }),
                            back_component,
                        )
                        .with_routing_receipt_request(routing_receipt_request_opt.clone()),
                    );
                }
                None => hops.push(LiveHop::new(&next_key, None, back_component)),
            }
        }
    }

    fn validate_route_segments(
        over: &RouteSegment,
        back: &Option<RouteSegment>,
    ) -> Option<RouteError> {
        if over.keys.is_empty() {
            return Some(RouteError::TooFewKeysInRouteSegment);
        }

        if let Some(b) = back {
            if b.keys.is_empty() {
                return Some(RouteError::TooFewKeysInRouteSegment);
            }
            let over_segment_last_key = &over.keys[over.keys.len() - 1];
            let back_segment_first_key = &b.keys[0];
            if back_segment_first_key != over_segment_last_key {
                return Some(RouteError::DisjointRouteSegments);
            }
        };
        None
    }

    fn hops_to_route(
        hops: Vec<LiveHop>,
        top_hop_key: &PublicKey,
        cryptde: &dyn CryptDE,
    ) -> Result<Route, CodexError> {
        let mut hops_enc: Vec<CryptData> = Vec::new();
        let mut hop_key = top_hop_key;
        for data_hop in &hops {
            hops_enc.push(match data_hop.encode(hop_key, cryptde) {
                Ok(crypt_data) => crypt_data,
                Err(e) => return Err(e),
            });
            hop_key = &data_hop.public_key;
        }
        Ok(Route { hops: hops_enc })
    }
}

pub struct RouteSegment {
    pub keys: Vec<PublicKey>,
    pub recipient: Component,
}

impl Debug for RouteSegment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RouteSegment {{ key_count: {}, recipient: {:?}, keys: [REDACTED] }}",
            self.keys.len(),
            self.recipient
        )
    }
}

impl RouteSegment {
    pub fn new(keys: Vec<&PublicKey>, recipient: Component) -> RouteSegment {
        RouteSegment {
            keys: keys.iter().map(|k| (*k).clone()).collect(),
            recipient,
        }
    }
}

#[derive(PartialEq, Eq)]
pub enum RouteError {
    HopDecodeProblem(String),
    EmptyRoute,
    TooFewKeysInRouteSegment,
    DisjointRouteSegments,
}

impl Debug for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::HopDecodeProblem(_) => "HopDecodeProblem([REDACTED])",
            Self::EmptyRoute => "EmptyRoute",
            Self::TooFewKeysInRouteSegment => "TooFewKeysInRouteSegment",
            Self::DisjointRouteSegments => "DisjointRouteSegments",
        })
    }
}

fn codex_error_category(error: &CodexError) -> &'static str {
    match error {
        CodexError::SerializationError(_) => "serialization",
        CodexError::DeserializationError(_) => "deserialization",
        CodexError::EncryptionError(_) => "encryption",
        CodexError::DecryptionError(_) => "decryption",
        CodexError::RoutingError(_) => "routing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrapper::CryptDEPair;
    use crate::sub_lib::cryptde_null::CryptDENull;
    use crate::sub_lib::service_receipt::{ReceiptSessionPolicy, ReceiptSessionRequest};
    use crate::test_utils::make_paying_wallet;
    use crate::test_utils::make_wallet;
    use lazy_static::lazy_static;
    use masq_lib::test_utils::utils::TEST_DEFAULT_CHAIN;
    use serde_cbor;

    lazy_static! {
        static ref CRYPTDE_PAIR: CryptDEPair = CryptDEPair::null();
    }

    #[test]
    fn construct_does_not_like_route_segments_with_too_few_keys() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_wallet("wallet");
        let result = Route::one_way(
            RouteSegment::new(vec![], Component::ProxyClient),
            cryptde,
            Some(paying_wallet.clone()),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .err()
        .unwrap();

        assert_eq!(
            result,
            CodexError::RoutingError(RouteError::TooFewKeysInRouteSegment)
        );
    }

    #[test]
    fn construct_does_not_like_route_segments_that_start_where_the_previous_segment_didnt_end() {
        let a_key = PublicKey::new(&[65, 65, 65]);
        let b_key = PublicKey::new(&[66, 66, 66]);
        let c_key = PublicKey::new(&[67, 67, 67]);
        let d_key = PublicKey::new(&[68, 68, 68]);
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");

        let result = Route::round_trip(
            RouteSegment::new(vec![&a_key, &b_key], Component::ProxyClient),
            RouteSegment::new(vec![&c_key, &d_key], Component::ProxyServer),
            cryptde,
            Some(paying_wallet.clone()),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .err()
        .unwrap();

        assert_eq!(
            result,
            CodexError::RoutingError(RouteError::DisjointRouteSegments)
        );
    }

    #[test]
    fn construct_can_make_single_hop_route() {
        let target_key = PublicKey::new(&[65, 65, 65]);
        let cryptde = CRYPTDE_PAIR.main.as_ref();

        let subject = Route::single_hop(&target_key, cryptde).unwrap();

        assert_eq!(2, subject.hops.len());
        assert_eq!(
            subject.hops[0],
            LiveHop::new(&target_key, None, Component::Hopper)
                .encode(&cryptde.public_key(), cryptde)
                .unwrap()
        );
        assert_eq!(
            subject.hops[1],
            LiveHop::new(&PublicKey::new(b""), None, Component::Neighborhood)
                .encode(&target_key, cryptde)
                .unwrap()
        );
    }

    #[test]
    fn construct_can_make_long_multistop_route() {
        let a_key = PublicKey::new(&[65, 65, 65]);
        let b_key = PublicKey::new(&[66, 66, 66]);
        let c_key = PublicKey::new(&[67, 67, 67]);
        let d_key = PublicKey::new(&[68, 68, 68]);
        let e_key = PublicKey::new(&[69, 69, 69]);
        let f_key = PublicKey::new(&[70, 70, 70]);
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;

        let subject = Route::round_trip(
            RouteSegment::new(vec![&a_key, &b_key, &c_key, &d_key], Component::ProxyClient),
            RouteSegment::new(vec![&d_key, &e_key, &f_key, &a_key], Component::ProxyServer),
            cryptde,
            Some(paying_wallet.clone()),
            Some(contract_address.clone()),
        )
        .unwrap();

        assert_eq!(
            subject.hops[0],
            LiveHop::new(
                &b_key,
                Some(paying_wallet.as_payer(&a_key, &contract_address)),
                Component::Hopper
            )
            .encode(&a_key, cryptde)
            .unwrap(),
            "first hop"
        );

        assert_eq!(
            subject.hops[1],
            LiveHop::new(
                &c_key,
                Some(paying_wallet.as_payer(&b_key, &contract_address)),
                Component::Hopper
            )
            .encode(&b_key, cryptde)
            .unwrap(),
            "second hop"
        );

        assert_eq!(
            subject.hops[2],
            LiveHop::new(
                &d_key,
                Some(paying_wallet.as_payer(&c_key, &contract_address)),
                Component::Hopper
            )
            .encode(&c_key, cryptde)
            .unwrap(),
            "third hop"
        );

        assert_eq!(
            subject.hops[3],
            LiveHop::new(
                &e_key,
                Some(paying_wallet.as_payer(&d_key, &contract_address)),
                Component::ProxyClient
            )
            .encode(&d_key, cryptde)
            .unwrap(),
            "fourth hop"
        );

        assert_eq!(
            subject.hops[4],
            LiveHop::new(
                &f_key,
                Some(paying_wallet.as_payer(&e_key, &contract_address)),
                Component::Hopper
            )
            .encode(&e_key, cryptde)
            .unwrap(),
            "fifth hop"
        );

        assert_eq!(
            subject.hops[5],
            LiveHop::new(
                &a_key,
                Some(paying_wallet.as_payer(&f_key, &contract_address)),
                Component::Hopper
            )
            .encode(&f_key, cryptde)
            .unwrap(),
            "sixth hop"
        );

        let empty_public_key = PublicKey::new(b"");
        assert_eq!(
            subject.hops[6],
            LiveHop::new(
                &empty_public_key,
                Some(paying_wallet.as_payer(&a_key, &contract_address)),
                Component::ProxyServer,
            )
            .encode(&a_key, cryptde)
            .unwrap(),
            "seventh hop"
        );
    }

    #[test]
    fn construct_can_make_short_single_stop_route() {
        let a_key = PublicKey::new(&[65, 65, 65]);
        let b_key = PublicKey::new(&[66, 66, 66]);
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;

        let subject = Route::one_way(
            RouteSegment::new(vec![&a_key, &b_key], Component::Neighborhood),
            cryptde,
            Some(paying_wallet.clone()),
            Some(contract_address.clone()),
        )
        .unwrap();
        let empty_public_key = PublicKey::new(b"");

        assert_eq!(
            vec!(
                LiveHop::new(
                    &b_key,
                    Some(paying_wallet.as_payer(&a_key, &contract_address)),
                    Component::Hopper
                )
                .encode(&a_key, cryptde)
                .unwrap(),
                LiveHop::new(
                    &empty_public_key,
                    Some(paying_wallet.as_payer(&b_key, &contract_address)),
                    Component::Neighborhood,
                )
                .encode(&b_key, cryptde)
                .unwrap(),
            ),
            subject.hops,
        );
    }

    #[test]
    fn next_hop_decodes_top_hop() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let key12 = cryptde.public_key();
        let key34 = PublicKey::new(&[3, 4]);
        let key56 = PublicKey::new(&[5, 6]);
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;
        let subject = Route::one_way(
            RouteSegment::new(vec![&key12, &key34, &key56], Component::Neighborhood),
            cryptde,
            Some(paying_wallet.clone()),
            Some(contract_address),
        )
        .unwrap();

        let next_hop = subject.next_hop(cryptde).unwrap();

        assert_eq!(
            next_hop,
            LiveHop::new(
                &key34,
                Some(paying_wallet.as_payer(&key12, &contract_address)),
                Component::Hopper
            )
        );
        let empty_public_key = PublicKey::new(b"");
        assert_eq!(
            subject.hops,
            vec!(
                LiveHop::new(
                    &key34,
                    Some(paying_wallet.as_payer(&key12, &contract_address)),
                    Component::Hopper
                )
                .encode(&key12, cryptde)
                .unwrap(),
                LiveHop::new(
                    &key56,
                    Some(paying_wallet.as_payer(&key34, &contract_address)),
                    Component::Hopper
                )
                .encode(&key34, cryptde)
                .unwrap(),
                LiveHop::new(
                    &empty_public_key,
                    Some(paying_wallet.as_payer(&key56, &contract_address)),
                    Component::Neighborhood,
                )
                .encode(&key56, cryptde)
                .unwrap(),
            )
        );
    }

    #[test]
    fn shift_returns_next_hop_and_adds_garbage_at_the_bottom() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let key12 = cryptde.public_key();
        let key34 = PublicKey::new(&[3, 4]);
        let key56 = PublicKey::new(&[5, 6]);
        let contract_address = TEST_DEFAULT_CHAIN.rec().contract;
        let mut subject = Route::one_way(
            RouteSegment::new(vec![&key12, &key34, &key56], Component::Neighborhood),
            cryptde,
            Some(paying_wallet.clone()),
            Some(contract_address),
        )
        .unwrap();
        let top_hop_len = subject.hops.first().unwrap().len();

        let next_hop = subject.shift(cryptde).unwrap();

        assert_eq!(
            next_hop,
            LiveHop::new(
                &key34,
                Some(paying_wallet.as_payer(&key12, &contract_address)),
                Component::Hopper
            )
        );
        let mut garbage_can: Vec<u8> = iter::repeat(0u8).take(top_hop_len).collect();
        cryptde.random(&mut garbage_can[..]);
        let empty_public_key = PublicKey::new(b"");
        assert_eq!(
            subject.hops,
            vec!(
                LiveHop::new(
                    &key56,
                    Some(paying_wallet.as_payer(&key34, &contract_address)),
                    Component::Hopper
                )
                .encode(&key34, cryptde)
                .unwrap(),
                LiveHop::new(
                    &empty_public_key,
                    Some(paying_wallet.as_payer(&key56, &contract_address)),
                    Component::Neighborhood,
                )
                .encode(&key56, cryptde)
                .unwrap(),
                CryptData::new(&garbage_can[..])
            )
        )
    }

    #[test]
    fn empty_route_says_none_when_asked_for_next_hop() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let subject = Route { hops: Vec::new() };

        let result = subject.next_hop(cryptde).err().unwrap();

        assert_eq!(result, CodexError::RoutingError(RouteError::EmptyRoute));
    }

    #[test]
    fn shift_says_none_when_asked_for_next_hop_on_empty_route() {
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let mut subject = Route { hops: Vec::new() };

        let result = subject.shift(cryptde).err().unwrap();

        assert_eq!(result, CodexError::RoutingError(RouteError::EmptyRoute));
    }

    #[test]
    fn route_serialization_deserialization() {
        let key1 = PublicKey::new(&[1, 2, 3, 4]);
        let key2 = PublicKey::new(&[4, 3, 2, 1]);
        let cryptde = CRYPTDE_PAIR.main.as_ref();
        let paying_wallet = make_paying_wallet(b"wallet");
        let original = Route::round_trip(
            RouteSegment::new(vec![&key1, &key2], Component::ProxyClient),
            RouteSegment::new(vec![&key2, &key1], Component::ProxyServer),
            cryptde,
            Some(paying_wallet),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();

        let serialized = serde_cbor::ser::to_vec(&original).unwrap();

        let deserialized = serde_cbor::de::from_slice::<Route>(&serialized[..]).unwrap();

        assert_eq!(deserialized, original);
    }

    #[test]
    fn route_debug_redacts_hops_keys_and_decode_details() {
        let route = Route {
            hops: vec![CryptData::new(b"encrypted route marker")],
        };
        let key = PublicKey::new(b"route segment identity marker");
        let segment = RouteSegment::new(vec![&key], Component::ProxyClient);
        let error = RouteError::HopDecodeProblem("route decode detail marker".to_string());

        assert_eq!(
            format!("{:?}", route),
            "Route { hop_count: 1, encrypted_hops: [REDACTED] }"
        );
        assert_eq!(
            format!("{:?}", segment),
            "RouteSegment { key_count: 1, recipient: ProxyClient, keys: [REDACTED] }"
        );
        assert_eq!(format!("{:?}", error), "HopDecodeProblem([REDACTED])");
    }

    #[test]
    fn to_string_works_with_one_way_route() {
        let key1 = PublicKey::new(&[1, 2, 3, 4]);
        let key2 = PublicKey::new(&[2, 3, 4, 5]);
        let key3 = PublicKey::new(&[3, 4, 5, 6]);
        let paying_wallet = make_paying_wallet(b"wallet");
        let subject = Route::one_way(
            RouteSegment::new(vec![&key1, &key2, &key3], Component::Neighborhood),
            CRYPTDE_PAIR.main.as_ref(),
            Some(paying_wallet),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();

        let result = subject.to_string(vec![
            &CryptDENull::from(&key1, TEST_DEFAULT_CHAIN),
            &CryptDENull::from(&key2, TEST_DEFAULT_CHAIN),
            &CryptDENull::from(&key3, TEST_DEFAULT_CHAIN),
        ]);

        assert_eq!(result.matches("Encrypted hop:").count(), 3);
        assert_eq!(result.matches("public_key: [REDACTED]").count(), 3);
        assert_eq!(result.matches("payer_present: true").count(), 3);
        assert!(result.contains("component: Neighborhood"));
        assert!(!result.contains("0x01020304"));
        assert!(!result.contains("proof:"));
        assert!(!result.contains("wallet:"));
    }

    #[test]
    fn to_string_works_with_round_trip_route() {
        let key1 = PublicKey::new(&[1, 2, 3, 4]);
        let key2 = PublicKey::new(&[2, 3, 4, 5]);
        let key3 = PublicKey::new(&[3, 4, 5, 6]);
        let cryptde = CryptDENull::from(&key1, TEST_DEFAULT_CHAIN);
        let paying_wallet = make_paying_wallet(b"wallet");
        let subject = Route::round_trip(
            RouteSegment::new(vec![&key1, &key2, &key3], Component::ProxyClient),
            RouteSegment::new(vec![&key3, &key2, &key1], Component::ProxyServer),
            &cryptde,
            Some(paying_wallet),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
        )
        .unwrap();

        let result = subject.to_string(vec![
            &CryptDENull::from(&key1, TEST_DEFAULT_CHAIN),
            &CryptDENull::from(&key2, TEST_DEFAULT_CHAIN),
            &CryptDENull::from(&key3, TEST_DEFAULT_CHAIN),
            &CryptDENull::from(&key2, TEST_DEFAULT_CHAIN),
            &CryptDENull::from(&key1, TEST_DEFAULT_CHAIN),
            &CryptDENull::from(&key1, TEST_DEFAULT_CHAIN),
        ]);

        assert_eq!(result.matches("Encrypted hop:").count(), 5);
        assert_eq!(result.matches("public_key: [REDACTED]").count(), 5);
        assert_eq!(result.matches("payer_present: true").count(), 5);
        assert!(result.contains("component: ProxyClient"));
        assert!(result.contains("component: ProxyServer"));
        assert!(!result.contains("0x01020304"));
        assert!(!result.contains("proof:"));
        assert!(!result.contains("wallet:"));
    }

    #[test]
    fn to_string_works_with_zero_length_data() {
        let subject = Route { hops: vec![] };

        let result = subject.to_string(vec![]);

        assert_eq!(result, String::from("\n"));
    }

    #[test]
    fn receipt_authorization_is_onion_encrypted_into_every_billable_round_trip_hop() {
        let origin = PublicKey::new(b"receipt route origin");
        let relay = PublicKey::new(b"receipt route relay");
        let exit = PublicKey::new(b"receipt route exit");
        let origin_cryptde = CryptDENull::from(&origin, TEST_DEFAULT_CHAIN);
        let payer_session_public_key = PublicKey::new(b"receipt route payer session");
        let wallet = make_paying_wallet(b"receipt route wallet");
        let authorization = ReceiptSessionPolicy::new(
            TEST_DEFAULT_CHAIN.rec().num_chain_id,
            TEST_DEFAULT_CHAIN.rec().contract,
            wallet.address(),
            payer_session_public_key,
            1_000_000,
            0,
            86_400,
            [0x61; 32],
        )
        .authorize(&wallet)
        .unwrap();
        let request = ReceiptSessionRequest::new(authorization, [0x62; 32]).unwrap();
        let route = Route::round_trip_with_receipts(
            RouteSegment::new(vec![&origin, &relay, &exit], Component::ProxyClient),
            RouteSegment::new(vec![&exit, &relay, &origin], Component::ProxyServer),
            &origin_cryptde,
            Some(wallet),
            Some(TEST_DEFAULT_CHAIN.rec().contract),
            Some(request.clone()),
        )
        .unwrap();
        let decrypting_keys = vec![origin.clone(), relay.clone(), exit.clone(), relay, origin];

        for (encrypted_hop, decrypting_key) in route.hops.iter().zip(decrypting_keys.into_iter()) {
            let cryptde = CryptDENull::from(&decrypting_key, TEST_DEFAULT_CHAIN);
            let hop = LiveHop::decode(&cryptde, encrypted_hop).unwrap();
            assert_eq!(hop.routing_receipt_request_opt, Some(request.clone()));
        }
    }
}
