use crate::ics02_client::client_type::ClientType;
use crate::ics02_client::header;
use crate::ics02_client::height::Height;
use crate::ics28_wasm::error::Error;
use ibc_proto::ibc::lightclients::wasm::v1::Header as RawHeader;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use tendermint_proto::Protobuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub height: Height,
    pub data: Vec<u8>,
}

impl header::Header for Header {
    fn client_type(&self) -> ClientType {
        ClientType::Wasm
    }
    fn height(&self) -> Height {
        self.height
    }
    fn wrap_any(self) -> header::AnyHeader {
        header::AnyHeader::Wasm(self)
    }
}

impl Protobuf<RawHeader> for Header {}

impl TryFrom<RawHeader> for Header {
    type Error = Error;
    fn try_from(raw: RawHeader) -> Result<Self, Error> {
        let h = raw.height.ok_or_else(|| {
            Error::missing_raw_client_state(String::from("missing latest_height"))
        })?;
        let s = Self {
            height: h.into(),
            data: raw.data,
        };
        Ok(s)
    }
}

impl From<Header> for RawHeader {
    fn from(h: Header) -> Self {
        Self {
            data: h.data,
            height: Some(h.height.into()),
        }
    }
}
