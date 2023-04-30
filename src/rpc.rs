use std::collections::vec_deque::VecDeque;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicUsize;

use codec::Encode;
use ecies_ed25519::encrypt;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::core::JsonValue;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use primitive_types::{H256, U256};
use rand::{rngs::StdRng, SeedableRng};
use schnorrkel::{ExpansionMode, MiniSecretKey, SecretKey, Signature};
use serde::Serialize;

#[derive(Clone)]
pub(crate) struct MiningParams {
    pub(crate) pre_hash: H256,
    pub(crate) parent_hash: H256,
    pub(crate) pow_difficulty: U256,
    pub(crate) pub_key: ecies_ed25519::PublicKey,
}

#[derive(Clone, Encode)]
pub(crate) enum AlgoType {
    Grid2d,
    Grid2dV2,
    Grid2dV3,
}

impl AlgoType {
    pub(crate) fn as_p3d_algo(&self) -> p3d::AlgoType {
        match self {
            Self::Grid2d => p3d::AlgoType::Grid2d,
            Self::Grid2dV2 => p3d::AlgoType::Grid2dV2,
            Self::Grid2dV3 => p3d::AlgoType::Grid2dV3,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Grid2d => "Grid2d",
            Self::Grid2dV2 => "Grid2dV2",
            Self::Grid2dV3 => "Grid2dV3",
        }
    }
}

#[derive(Clone)]
pub(crate) struct P3dParams {
    pub(crate) algo: AlgoType,
    pub(crate) grid: usize,
    pub(crate) sect: usize,
}

impl P3dParams {
    pub(crate) fn new(ver: &str) -> Self {
        let grid = 8;
        let (algo, sect) = if ver == "grid2d" {
            (AlgoType::Grid2d, 66)
        } else if ver == "grid2d_v2" {
            (AlgoType::Grid2dV2, 12)
        } else if ver == "grid2d_v3" {
            (AlgoType::Grid2dV3, 12)
        } else {
            panic!("Unknown algorithm: {}", ver)
        };

        Self { algo, grid, sect }
    }
}

pub(crate) struct MiningObj {
    pub(crate) obj_id: u64,
    pub(crate) obj: Vec<u8>,
}

pub(crate) struct MiningProposal {
    pub(crate) params: MiningParams,
    pub(crate) hash: H256,
    pub(crate) obj_id: u64,
    pub(crate) obj: Vec<u8>,
}

#[derive(Serialize)]
pub(crate) struct Payload {
    pub(crate) pool_id: String,
    pub(crate) member_id: String,
    pub(crate) pre_hash: H256,
    pub(crate) parent_hash: H256,
    pub(crate) algo: String,
    pub(crate) dfclty: U256,
    pub(crate) hash: H256,
    pub(crate) obj_id: u64,
    pub(crate) obj: Vec<u8>,
}

pub(crate) struct MiningContext {
    pub(crate) p3d_params: P3dParams,
    pub(crate) pool_id: String,
    pub(crate) member_id: String,
    pub(crate) key: SecretKey,
    pub(crate) cur_state: Mutex<Option<MiningParams>>,
    pub(crate) out_queue: Mutex<VecDeque<MiningProposal>>,
    pub(crate) iterations_count: Arc<AtomicUsize>,
    pub(crate) bad_objects: Arc<AtomicUsize>,
    pub(crate) dupe_objects: Arc<AtomicUsize>,
    pub(crate) seen_objects: Mutex<std::collections::HashSet<H256>>,

    pub(crate) client: HttpClient,
}

impl MiningContext {
    pub(crate) fn new(
        p3d_params: P3dParams,
        pool_addr: &str,
        pool_id: String,
        member_id: String,
        key: String,
    ) -> anyhow::Result<Self> {
        let key = key.replacen("0x", "", 1);
        let key_data = hex::decode(&key[..])?;
        let key = MiniSecretKey::from_bytes(&key_data[..])
            .expect("Invalid key")
            .expand(ExpansionMode::Ed25519);

        Ok(MiningContext {
            p3d_params,
            pool_id,
            member_id,
            key,
            cur_state: Mutex::new(None),
            out_queue: Mutex::new(VecDeque::new()),
            iterations_count: Arc::new(AtomicUsize::new(0)),
            bad_objects: Arc::new(AtomicUsize::new(0)),
            dupe_objects: Arc::new(AtomicUsize::new(0)),
            seen_objects: Mutex::new(std::collections::HashSet::new()),
            client: HttpClientBuilder::default().build(pool_addr)?,
        })
    }

    pub(crate) fn push_to_queue(&self, prosal: MiningProposal) {
        let mut lock = self.out_queue.lock().unwrap();
        (*lock).push_back(prosal);
    }

    pub(crate) async fn ask_mining_params(&self) -> anyhow::Result<()> {
        println!("Ask for mining params...");

        let response: JsonValue = self
            .client
            .request(
                "poscan_getMiningParams",
                rpc_params![serde_json::json!(self.pool_id)],
            )
            .await?;

        let pre_hash: Option<&str> = response.get(0).expect("Expect pre_hash").as_str();
        let parent_hash: Option<&str> = response.get(1).expect("Expect parent_hash").as_str();
        let pow_difficulty: Option<&str> = response.get(3).expect("Expect pow_difficulty").as_str();
        let pub_key: Option<&str> = response.get(4).expect("public key").as_str();

        match (pre_hash, parent_hash, pow_difficulty, pub_key) {
            (
                Some(pre_hash),
                Some(parent_hash),
                Some(pow_difficulty),
                Some(pub_key),
            ) => {
                let pre_hash = H256::from_str(pre_hash).unwrap();
                let parent_hash = H256::from_str(parent_hash).unwrap();
                let pow_difficulty = U256::from_str_radix(pow_difficulty, 16).unwrap();
                let pub_key = U256::from_str_radix(pub_key, 16).unwrap();
                let mut pub_key = pub_key.encode();
                pub_key.reverse();
                let pub_key = ecies_ed25519::PublicKey::from_bytes(&pub_key).unwrap();

                let mut lock = self.cur_state.lock().unwrap();
                (*lock) = Some(MiningParams {
                    pre_hash,
                    parent_hash,
                    pow_difficulty: pow_difficulty,
                    pub_key,
                });
                println!("Mining params applied. Pow difficulty: {}, pre_hash: {}, parent_hash: {}", pow_difficulty, pre_hash, parent_hash);
            }
            _ => {
                println!("🟥 Ask_mining_params error: Incorrect response from poll node");
            }
        }
        Ok(())
    }

    pub(crate) async fn push_to_node(&self, proposal: MiningProposal) -> anyhow::Result<()> {
        println!("📦 Pushing obj to node...");

        let payload = Payload {
            pool_id: self.pool_id.clone(),
            member_id: self.member_id.clone(),
            pre_hash: proposal.params.pre_hash,
            parent_hash: proposal.params.parent_hash,
            algo: self.p3d_params.algo.as_str().into(),
            dfclty: proposal.params.pow_difficulty,
            hash: proposal.hash,
            obj_id: proposal.obj_id,
            obj: proposal.obj,
        };

        let message = serde_json::to_string(&payload).unwrap();
        let mut csprng = StdRng::from_seed(proposal.hash.encode().try_into().unwrap());
        let encrypted = encrypt(&proposal.params.pub_key, message.as_bytes(), &mut csprng).unwrap();
        let sign = self.sign(&encrypted);

        let params = rpc_params![
            serde_json::json!(encrypted.clone()),
            serde_json::json!(self.member_id.clone()),
            serde_json::json!(hex::encode(sign.to_bytes()))
        ];

        let _response: JsonValue = self
            .client
            .request("poscan_pushMiningObjectToPool", params)
            .await?;

        Ok(())
    }

    fn sign(&self, msg: &[u8]) -> Signature {
        const CTX: &[u8] = b"Mining pool";
        self.key.sign_simple(CTX, msg, &self.key.to_public())
    }
}
