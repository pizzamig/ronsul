use super::Opt;
use crate::error::RonsulError;
use futures::{Async, Future, Poll, Stream};
use reqwest::r#async::{Chunk, Client as ReqwClient, Decoder, Response as ReqwResponse};
use reqwest::Url as ReqwUrl;
use reqwest::{Error as ReqwError, StatusCode};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use trust_dns_resolver::Resolver;

pub fn get_consul_ips(opt: &Opt) -> Result<HashSet<IpAddr>, RonsulError> {
    let mut rv = HashSet::new();
    if opt.consul_server.is_some() {
        rv.insert(opt.consul_server.unwrap());
    } else {
        let resolver = Resolver::from_system_conf().map_err(RonsulError::ResolverConf)?;
        //            .with_context(|_| "wtf, your /etc/resolv.conf is unreadable")?;
        loop {
            let start_len = rv.len();
            let response = resolver
                .ipv4_lookup("consul.service.consul.")
                .map_err(RonsulError::Resolver)?;
            //                .with_context(|e| {
            //                    format!("failed to resolve consul.service.consul (error {})", e)
            //                })?;
            response.into_iter().for_each(|x| {
                rv.insert(IpAddr::V4(x));
            });
            if start_len == rv.len() {
                break;
            }
        }
    }
    Ok(rv)
}

type ServiceMap = HashMap<String, HashSet<String>>;

pub fn from_json_to_service_map(value: &JsonValue) -> Result<ServiceMap, RonsulError> {
    if let JsonValue::Object(ref map) = value {
        let mut out: ServiceMap = HashMap::with_capacity(map.len());
        for (k, v) in map.iter() {
            if v.is_array() {
                if k != "consul" {
                    let tag_set: HashSet<String> = serde_json::from_value(v.to_owned())
                        .map_err(RonsulError::JsonDeserialize)?;
                    out.insert(k.to_string(), tag_set);
                }
            } else {
                return Err(RonsulError::UnexpectedJsonFormat(
                    "Services should be listed in an array",
                ));
            }
        }
        Ok(out)
    } else {
        return Err(RonsulError::UnexpectedJsonFormat(
            "Service list should be a JSON object",
        ));
    }
}

pub struct ConsulCatalogService {
    base: ReqwUrl,
    state: ReqwFutureState,
    block_index: u64,
    chunk: Chunk,
}

enum ReqwFutureState {
    Init,
    Get(Box<Future<Item = ReqwResponse, Error = ReqwError> + Send>),
    GetBody(Box<Decoder>),
}

impl Future for ConsulCatalogService {
    type Item = (ServiceMap, u64);
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            match &mut self.state {
                ReqwFutureState::Init => {
                    let url = self
                        .base
                        .join(&format!(
                            "/v1/catalog/services?stale&index={}",
                            self.block_index
                        ))
                        .unwrap();

                    let future = ReqwClient::builder().build().unwrap().get(url).send();
                    self.state = ReqwFutureState::Get(Box::new(future));
                }
                ReqwFutureState::Get(b) => {
                    match b.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Err(_) => (),
                        Ok(Async::Ready(response)) => {
                            if response.status() == StatusCode::OK {
                                let hm = response.headers();
                                if hm.contains_key("x-consul-index") {
                                    self.block_index = hm
                                        .get("x-consul-index")
                                        .unwrap()
                                        .to_str()
                                        .unwrap()
                                        .parse()
                                        .unwrap_or(1);
                                }
                            }
                            self.state = ReqwFutureState::GetBody(Box::new(response.into_body()));
                        }
                    };
                }
                ReqwFutureState::GetBody(b) => match b.poll() {
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Err(_) => (),
                    Ok(Async::Ready(Some(data))) => {
                        self.chunk.extend(data);
                    }
                    Ok(Async::Ready(None)) => {
                        let v: JsonValue = serde_json::from_slice(&self.chunk).unwrap();
                        let rv = from_json_to_service_map(&v).unwrap();
                        return Ok(Async::Ready((rv, self.block_index)));
                    }
                },
            }
        }
    }
}

pub fn get_catalog_services(base: ReqwUrl, block_index: u64) -> ConsulCatalogService {
    ConsulCatalogService {
        base,
        state: ReqwFutureState::Init,
        block_index,
        chunk: Chunk::default(),
    }
}

pub struct ConsulCatalogServiceUpdates {
    base: ReqwUrl,
    state: ReqwStreamState,
    block_index: u64,
    current_services: ServiceMap,
}

enum ReqwStreamState {
    Init,
    Get(Box<ConsulCatalogService>),
}

impl Stream for ConsulCatalogServiceUpdates {
    type Item = (ServiceMap, ServiceMap);
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            match &mut self.state {
                ReqwStreamState::Init => {
                    let new_req = get_catalog_services(self.base.clone(), self.block_index);
                    self.state = ReqwStreamState::Get(Box::new(new_req));
                }
                ReqwStreamState::Get(f) => match f.poll() {
                    Err(_) => (),
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Ok(Async::Ready((new_services, block_index))) => {
                        println!("Here Get - Ready!");
                        let mut added = HashMap::new();
                        let mut removed = HashMap::new();
                        for (new_service, new_tags) in new_services.iter() {
                            if !self.current_services.contains_key(new_service) {
                                added.insert(new_service.clone(), new_tags.clone());
                            } else if self.current_services.get(new_service).unwrap() != new_tags {
                                println!("Tag change detected: service name {}", new_service);
                                println!(
                                    "Old tags: {:?}",
                                    self.current_services.get(new_service).unwrap()
                                );
                                println!("New tags: {:?}", new_tags);
                            }
                        }
                        for (old_service, old_tags) in self.current_services.iter() {
                            if !new_services.contains_key(old_service) {
                                removed.insert(old_service.clone(), old_tags.clone());
                            }
                        }
                        std::mem::replace(&mut self.current_services, new_services);
                        self.state = ReqwStreamState::Init;
                        if block_index < self.block_index {
                            self.block_index = 0;
                        } else {
                            self.block_index = block_index;
                        }
                        println!("Here Get - New block index: {}!", block_index);
                        return Ok(Async::Ready(Some((added, removed))));
                    }
                },
            }
        }
    }
}

pub fn get_catalog_services_updates(base: ReqwUrl) -> ConsulCatalogServiceUpdates {
    ConsulCatalogServiceUpdates {
        base,
        state: ReqwStreamState::Init,
        block_index: 0,
        current_services: HashMap::new(),
    }
}
