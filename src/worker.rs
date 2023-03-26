use codec::Encode;
use primitive_types::{H256, U256};
use sha3::{Digest, Sha3_256};
use std::str::FromStr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::time;

use super::rpc::MiningParams;
use super::MiningContext;
use super::P3dParams;
use crate::rpc::MiningProposal;
use p3d::p3d_process;

const ASK_MINING_PRAMS_PERIOD: Duration = Duration::from_secs(10);

#[derive(Encode)]
pub struct DoubleHash {
    pub pre_hash: H256,
    pub obj_hash: H256,
}

impl DoubleHash {
    pub fn calc_hash(self) -> H256 {
        H256::from_slice(Sha3_256::digest(&self.encode()[..]).as_slice())
    }
}

#[derive(Clone, Encode)]
pub struct Compute {
    pub difficulty: U256,
    pub pre_hash: H256,
    pub poscan_hash: H256,
}

impl Compute {
    pub(crate) fn get_work(&self) -> H256 {
        H256::from_slice(Sha3_256::digest(&self.encode()[..]).as_slice())
    }
}

pub fn hash_meets_difficulty(hash: &H256, difficulty: U256) -> bool {
    let num_hash = U256::from(&hash[..]);
    let (_, overflowed) = num_hash.overflowing_mul(difficulty);

    !overflowed
}

pub(crate) fn worker(ctx: &MiningContext) {
    let P3dParams { algo, sect, grid } = ctx.p3d_params.clone();

    loop {
        let mining_params = {
            let params_lock = ctx.cur_state.lock().unwrap();
            if let Some(mp) = (*params_lock).clone() {
                mp
            } else {
                drop(params_lock);
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let mut obj_lock = ctx.in_queue.lock().unwrap();
        let obj = (*obj_lock).pop_front();
        drop(obj_lock);

        if let Some(mining_obj) = obj {
            let MiningParams {
                pre_hash,
                parent_hash,
                win_dfclty,
                pow_dfclty,
                ..
            } = mining_params;
            let pre = parent_hash.encode()[0..4].try_into().ok();

            let res_hashes = p3d_process(
                mining_obj.obj.as_slice(),
                algo.as_p3d_algo(),
                grid as i16,
                sect as i16,
                pre,
            );
            let first_hash = &res_hashes.unwrap()[0];
            let obj_hash = H256::from_str(first_hash).unwrap();
            println!("obj_hash = {}", &obj_hash.to_string());

            let poscan_hash = DoubleHash { pre_hash, obj_hash }.calc_hash();

            let comp = Compute {
                difficulty: pow_dfclty,
                pre_hash,
                poscan_hash,
            };

            if hash_meets_difficulty(&comp.get_work(), pow_dfclty) {
                let prop = MiningProposal {
                    params: mining_params.clone(),
                    hash: obj_hash.clone(),
                    obj_id: mining_obj.obj_id.clone(),
                    obj: mining_obj.obj.clone(),
                };
                ctx.push_to_queue(prop);
            }

            let comp = Compute {
                difficulty: win_dfclty,
                pre_hash,
                poscan_hash,
            };

            if hash_meets_difficulty(&comp.get_work(), win_dfclty) {
                let prop = MiningProposal {
                    params: mining_params.clone(),
                    hash: obj_hash.clone(),
                    obj_id: mining_obj.obj_id.clone(),
                    obj: mining_obj.obj.clone(),
                };
                ctx.push_to_queue(prop);
            }
        } else {
            thread::sleep(Duration::from_millis(100))
        }
    }
}

pub(crate) async fn node_client(ctx: Arc<MiningContext>) {
    loop {
        let maybe_prop = {
            let mut lock = ctx.out_queue.lock().unwrap();
            let maybe_prop = (*lock).pop_front();
            maybe_prop
        };
        if let Some(prop) = maybe_prop {
            let res = ctx.push_to_node(prop).await;
            if let Err(e) = res {
                println!("Error: {}", &e);
            }
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

pub(crate) fn start_timer(ctx: Arc<MiningContext>) {
    let _forever = tokio::spawn(async move {
        let mut interval = time::interval(ASK_MINING_PRAMS_PERIOD);

        loop {
            interval.tick().await;
            let _res = ctx.ask_mining_params().await;
        }
    });
}
