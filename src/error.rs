use failure_derive::Fail;
use serde_json::Error as JsonError;
use std::io::Error as IoError;
use trust_dns_resolver::error::ResolveError;

#[derive(Debug, Fail)]
pub enum RonsulError {
    #[fail(display = "resolver.conf error {}", _0)]
    ResolverConf(IoError),
    #[fail(display = "DNS resolver error {}", _0)]
    Resolver(ResolveError),
    #[fail(display = "JSON deserialization error {}", _0)]
    JsonDeserialize(JsonError),
    #[fail(display = "Unexecpted JSON format error {}", _0)]
    UnexpectedJsonFormat(&'static str),
}
