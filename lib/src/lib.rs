use secp256k1::ecdh::SharedSecret;
use secp256k1::ecdsa::Signature;
use secp256k1::hashes::sha256;
use secp256k1::rand::rngs::ThreadRng;
use secp256k1::rand::Rng;
use secp256k1::{rand, All, Message, PublicKey, Secp256k1};
use std::fmt;
use std::fmt::Debug;
// use bech32::{self, FromBase32, ToBase32, Variant};
use crate::commands::unzip;
pub mod apdu;
pub mod commands;
pub mod factory_root_key;

#[cfg(feature = "pcsc")]
pub mod pcsc;

use apdu::*;
use commands::*;

pub enum CkTapCard<T: CkTransport> {
    SatsCard(SatsCard<T>),
    TapSigner(TapSigner<T>),
    SatsChip(TapSigner<T>),
}

impl<T: CkTransport> fmt::Debug for CkTapCard<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            CkTapCard::TapSigner(t) => {
                write!(f, "CkTap::TapSigner({:?})", t)
            }
            CkTapCard::SatsChip(t) => {
                write!(f, "CkTap::SatsChip({:?})", t)
            }
            CkTapCard::SatsCard(s) => {
                write!(f, "CkTap::SatsCard({:?})", s)
            }
        }
    }
}

pub struct TapSigner<T: CkTransport> {
    pub transport: T,
    pub secp: Secp256k1<All>,
    pub proto: usize,
    pub ver: String,
    pub birth: usize,
    pub path: Option<Vec<usize>>,
    // [(1<<31)+84, (1<<31), (1<<31)], user-defined, will be omitted if not yet setup
    pub num_backups: Option<usize>,
    pub pubkey: PublicKey,
    pub card_nonce: Vec<u8>, // 16 bytes
    pub auth_delay: Option<usize>,
}

impl<T: CkTransport> Authentication<T> for TapSigner<T> {
    fn secp(&self) -> &Secp256k1<All> {
        &self.secp
    }

    fn pubkey(&self) -> &PublicKey {
        &self.pubkey
    }

    fn slot(&self) -> Option<u8> {
        None
    }

    fn card_nonce(&self) -> &Vec<u8> {
        &self.card_nonce
    }

    fn set_card_nonce(&mut self, new_nonce: Vec<u8>) {
        self.card_nonce = new_nonce;
    }

    fn auth_delay(&self) -> &Option<usize> {
        &self.auth_delay
    }

    fn set_auth_delay(&mut self, auth_delay: Option<usize>) {
        self.auth_delay = auth_delay;
    }

    fn transport(&self) -> &T {
        &self.transport
    }
}

impl<T: CkTransport> TapSigner<T> {
    pub fn from_status(transport: T, status_response: StatusResponse) -> Self {
        let pubkey = status_response.pubkey.as_slice(); // TODO verify is 33 bytes?
        let pubkey = PublicKey::from_slice(pubkey)
            .map_err(|e| Error::CiborValue(e.to_string()))
            .unwrap();
        Self {
            transport,
            secp: Secp256k1::new(),
            proto: status_response.proto,
            ver: status_response.ver,
            birth: status_response.birth,
            path: status_response.path,
            num_backups: status_response.num_backups,
            pubkey,
            card_nonce: status_response.card_nonce,
            auth_delay: status_response.auth_delay,
        }
    }

    pub fn init(&mut self, chain_code: Vec<u8>, cvc: String) -> Result<NewResponse, Error> {
        let chain_code = Some(chain_code);
        let (_, epubkey, xcvc) = self.calc_ekeys_xcvc(cvc, &NewCommand::name());
        let epubkey = epubkey.serialize().to_vec();
        let new_command = NewCommand::new(0, chain_code, epubkey, xcvc);
        let new_response: Result<NewResponse, Error> = self.transport.transmit(new_command);
        if let Ok(response) = &new_response {
            self.card_nonce = response.card_nonce.clone();
        }
        new_response
    }

    pub fn derive(&mut self, path: Vec<u32>, cvc: String) -> Result<DeriveResponse, Error> {
        // set most significant bit to 1 to represent hardened path steps
        let path = path.iter().map(|p| p ^ (1 << 31)).collect();
        let app_nonce = rand_nonce();
        let (eprivkey, epubkey, xcvc) = self.calc_ekeys_xcvc(cvc, &DeriveCommand::name());
        let cmd = DeriveCommand::for_tapsigner(app_nonce.clone(), path, epubkey, xcvc);
        let derive_response: Result<DeriveResponse, Error> = self.transport.transmit(cmd);
        if let Ok(response) = &derive_response {
            // self.card_nonce = response.card_nonce.clone();
            // TODO why doesn't below verify_sig work?
            // self.verify_sig(
            //     app_nonce,
            //     &response.sig,
            //     Some(&eprivkey),
            //     &response.pubkey.clone().expect("derive response pubkey"),
            //     &response.chain_code,
            // )?;
            let card_nonce = self.card_nonce();
            let sig = &response.sig;
            let eprivkey = Some(&eprivkey);
            let pubkey = &response.pubkey.clone().expect("derive response pubkey");
            let data = &response.chain_code;
            // let message = self.message_digest(app_nonce, data.to_owned());
            let mut message_bytes: Vec<u8> = Vec::new();
            message_bytes.extend("OPENDIME".as_bytes());
            message_bytes.extend(card_nonce);
            message_bytes.extend(app_nonce);
            message_bytes.extend(data);
            let message = Message::from_hashed_data::<sha256::Hash>(message_bytes.as_slice());
            
            let signature = Signature::from_compact(sig.as_slice())?;
            // let pubkey = if let Some(epk) = eprivkey {
            //     let session_key = SharedSecret::new(self.pubkey(), epk);
            //     unzip(&mut pubkey.clone(), session_key).unwrap()
            // } else {
            //     PublicKey::from_slice(pubkey.as_slice()).unwrap()
            // };
            let pubkey = PublicKey::from_slice(&response.master_pubkey.as_slice()).unwrap();

            self.secp()
                .verify_ecdsa(&message, &signature, &pubkey)?;
                // .map_err(|e| e.into())?;
            self.set_card_nonce(response.card_nonce.clone());
        }
        derive_response
    }
}

impl<T: CkTransport> Wait<T> for TapSigner<T> {}

impl<T: CkTransport> Read<T> for TapSigner<T> {
    fn requires_auth(&self) -> bool {
        true
    }
}

impl<T: CkTransport> Certificate<T> for TapSigner<T> {
    fn message_digest(&mut self, card_nonce: Vec<u8>, app_nonce: Vec<u8>) -> Message {
        let mut message_bytes: Vec<u8> = Vec::new();
        message_bytes.extend("OPENDIME".as_bytes());
        message_bytes.extend(card_nonce);
        message_bytes.extend(app_nonce);
        Message::from_hashed_data::<sha256::Hash>(message_bytes.as_slice())
    }
}

impl<T: CkTransport> Debug for TapSigner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TapSigner")
            .field("proto", &self.proto)
            .field("ver", &self.ver)
            .field("birth", &self.birth)
            .field("path", &self.path)
            .field("num_backups", &self.num_backups)
            .field("pubkey", &self.pubkey)
            .field("card_nonce", &self.card_nonce)
            .field("auth_delay", &self.auth_delay)
            .finish()
    }
}

pub struct SatsCard<T: CkTransport> {
    pub transport: T,
    pub secp: Secp256k1<All>,
    pub proto: usize,
    pub ver: String,
    pub birth: usize,
    pub slots: (usize, usize),
    pub addr: Option<String>,
    pub pubkey: PublicKey,
    pub card_nonce: Vec<u8>, // 16 bytes
    pub auth_delay: Option<usize>,
}

impl<T: CkTransport> Authentication<T> for SatsCard<T> {
    fn secp(&self) -> &Secp256k1<All> {
        &self.secp
    }

    fn pubkey(&self) -> &PublicKey {
        &self.pubkey
    }

    fn slot(&self) -> Option<u8> {
        Some(self.slots.0 as u8)
    }

    fn card_nonce(&self) -> &Vec<u8> {
        &self.card_nonce
    }

    fn set_card_nonce(&mut self, new_nonce: Vec<u8>) {
        self.card_nonce = new_nonce;
    }

    fn auth_delay(&self) -> &Option<usize> {
        &self.auth_delay
    }

    fn set_auth_delay(&mut self, auth_delay: Option<usize>) {
        self.auth_delay = auth_delay;
    }

    fn transport(&self) -> &T {
        &self.transport
    }
}

impl<T: CkTransport> SatsCard<T> {
    pub fn from_status(transport: T, status_response: StatusResponse) -> Result<Self, Error> {
        let pubkey = status_response.pubkey.as_slice(); // TODO verify is 33 bytes?
        let pubkey = PublicKey::from_slice(pubkey)
            .map_err(|e| Error::CiborValue(e.to_string()))
            .unwrap();
        let slots = status_response
            .slots
            .ok_or_else(|| Error::CiborValue("Missing slots".to_string()))?;
        Ok(Self {
            transport,
            secp: Secp256k1::new(),
            proto: status_response.proto,
            ver: status_response.ver,
            birth: status_response.birth,
            pubkey,
            card_nonce: status_response.card_nonce,
            auth_delay: status_response.auth_delay,
            slots,
            addr: status_response.addr,
        })
    }

    pub fn new_slot(
        &mut self,
        slot: usize,
        chain_code: Vec<u8>,
        cvc: String,
    ) -> Result<NewResponse, Error> {
        let chain_code = Some(chain_code);
        let (_, epubkey, xcvc) = self.calc_ekeys_xcvc(cvc, &NewCommand::name());
        let epubkey = epubkey.serialize().to_vec();
        let new_command = NewCommand::new(slot, chain_code, epubkey, xcvc);
        let new_response: Result<NewResponse, Error> = self.transport.transmit(new_command);
        if let Ok(response) = &new_response {
            self.card_nonce = response.card_nonce.clone();
        }
        new_response
    }

    pub fn derive(&mut self) -> Result<DeriveResponse, Error> {
        let cmd = DeriveCommand::for_satscard(self.card_nonce.clone());
        let resp: Result<DeriveResponse, Error> = self.transport.transmit(cmd);
        if let Ok(derive_resp) = &resp {
            self.card_nonce = derive_resp.card_nonce.clone();

            // TODO: verify reponse
            // The digest:
            // b'OPENDIME' (8 bytes)
            // (card_nonce - 16 bytes)
            // (nonce from command - 16 bytes)
            // (chain_code - 32 bytes)
            // must be signed by the master_pubkey
        }
        resp
    }

    pub fn unseal(&self, slot: usize, cvc: String) -> Result<UnsealResponse, Error> {
        let (_, epubkey, xcvc) = self.calc_ekeys_xcvc(cvc, &UnsealCommand::name());
        let epubkey = epubkey.serialize().to_vec();
        let unseal_command = UnsealCommand::new(slot, epubkey, xcvc);
        self.transport.transmit(unseal_command)
    }

    pub fn dump(&self, slot: usize, cvc: Option<String>) -> Result<DumpResponse, Error> {
        let epubkey_xcvc = cvc.map(|cvc| {
            let (_, epubkey, xcvc) = self.calc_ekeys_xcvc(cvc, &DumpCommand::name());
            (epubkey, xcvc)
        });

        let (epubkey, xcvc) = epubkey_xcvc
            .map(|(epubkey, xcvc)| (Some(epubkey.serialize().to_vec()), Some(xcvc)))
            .unwrap_or((None, None));

        let dump_command = DumpCommand::new(slot, epubkey, xcvc);
        self.transport.transmit(dump_command)
    }

    pub fn address(&mut self) -> Result<String, Error> {
        // let pubkey = self.read(None).unwrap().pubkey;
        // // let hrp = match self.testnet() { }
        // let encoded = bech32::encode("bc", pubkey.to_base32(), Variant::Bech32);
        // match encoded {
        //     Ok(e) => Ok(e),
        //     Err(_) => panic!("Failed to encoded pubkey")
        // }
        todo!()
    }
}

impl<T: CkTransport> Wait<T> for SatsCard<T> {}

impl<T: CkTransport> Read<T> for SatsCard<T> {
    fn requires_auth(&self) -> bool {
        false
    }
}

impl<T: CkTransport> Certificate<T> for SatsCard<T> {
    fn message_digest(&mut self, card_nonce: Vec<u8>, app_nonce: Vec<u8>) -> Message {
        let mut message_bytes: Vec<u8> = Vec::new();
        message_bytes.extend("OPENDIME".as_bytes());
        message_bytes.extend(card_nonce);
        message_bytes.extend(app_nonce);
        if self.ver != "0.9.0" {
            let pubkey = self.read(None).unwrap().pubkey;
            message_bytes.extend(pubkey);
        }
        Message::from_hashed_data::<sha256::Hash>(message_bytes.as_slice())
    }
}

impl<T: CkTransport> Debug for SatsCard<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SatsCard")
            .field("proto", &self.proto)
            .field("ver", &self.ver)
            .field("birth", &self.birth)
            .field("slots", &self.slots)
            .field("addr", &self.addr)
            .field("pubkey", &self.pubkey)
            .field("card_nonce", &self.card_nonce)
            .field("auth_delay", &self.auth_delay)
            .finish()
    }
}

// utility functions

pub fn rand_chaincode(rng: &mut ThreadRng) -> [u8; 32] {
    let mut chain_code = [0u8; 32];
    rng.fill(&mut chain_code);
    chain_code
}

pub fn rand_nonce() -> Vec<u8> {
    let rng = &mut rand::thread_rng();
    let mut nonce = [0u8; 16];
    rng.fill(&mut nonce);
    nonce.to_vec()
}
