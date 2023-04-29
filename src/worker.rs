use std::convert::TryInto;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use cgmath::{InnerSpace, Matrix4, Vector3, Vector4, VectorSpace};
use codec::Encode;
use genmesh::{MapVertex, Triangle, Triangulate};
use genmesh::generators::{IndexedPolygon, SharedVertex, SphereUv};
use p3d::p3d_process;
use primitive_types::{H256, U256};
use rand::prelude::*;
use sha3::{Digest, Sha3_256};
use tokio::time;

use crate::rpc::{MiningObj, MiningProposal};

use super::MiningContext;
use super::P3dParams;
use super::rpc::MiningParams;

const ASK_MINING_PARAMS_PERIOD: Duration = Duration::from_secs(10);

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
        let encoded_data = self.encode();
        let hash_digest = Sha3_256::digest(&encoded_data);
        H256::from_slice(&hash_digest)
    }
}

pub fn hash_meets_difficulty(hash: &H256, difficulty: U256) -> bool {
    let num_hash = U256::from(&hash[..]);
    !num_hash.overflowing_mul(difficulty).1
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

        let mining_obj: MiningObj = MiningObj {
            obj_id: 1,
            obj: create_mining_obj(),
        };

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

        // check if Result is Ok and if it contains at least one hash, otherwise continue
        if res_hashes.is_err() || res_hashes.as_ref().unwrap().len() == 0 {
            continue;
        }

        let first_hash = &res_hashes.unwrap()[0];
        let obj_hash = H256::from_str(first_hash).unwrap();

        let poscan_hash = DoubleHash { pre_hash, obj_hash }.calc_hash();

        let comp = Compute {
            difficulty: pow_dfclty,
            pre_hash,
            poscan_hash,
        };
        ctx.iterations_count.fetch_add(1, Ordering::Relaxed);

        if hash_meets_difficulty(&comp.get_work(), pow_dfclty) {
            let prop = MiningProposal {
                params: mining_params.clone(),
                hash: obj_hash,
                obj_id: mining_obj.obj_id,
                obj: mining_obj.obj.clone(),
            };
            ctx.push_to_queue(prop);
            println!("💎 Hash meets difficulty: {}", &pow_dfclty);
        }

        let comp = Compute {
            difficulty: win_dfclty,
            pre_hash,
            poscan_hash,
        };

        if hash_meets_difficulty(&comp.get_work(), win_dfclty) {
            let prop = MiningProposal {
                params: mining_params.clone(),
                hash: obj_hash,
                obj_id: mining_obj.obj_id,
                obj: mining_obj.obj,
            };
            ctx.push_to_queue(prop);
        }
    }
}

pub(crate) async fn node_client(ctx: Arc<MiningContext>) {
    loop {
        let maybe_prop = {
            let mut lock = ctx.out_queue.lock().unwrap();
            (*lock).pop_front()
        };
        if let Some(prop) = maybe_prop {
            let res = ctx.push_to_node(prop).await;
            if let Err(e) = res {
                println!("🟥 Error: {}", &e);
            }
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

pub(crate) fn start_timer(ctx: Arc<MiningContext>) {
    let _forever = tokio::spawn(async move {
        let mut interval = time::interval(ASK_MINING_PARAMS_PERIOD);

        let mut prev_iterations: usize = 0;
        let mut ema_iterations_per_second: f64 = 0.0;
        // EMA smoothing factor between 0 and 1; higher value means more smoothing
        let alpha: f64 = 0.8;

        loop {
            interval.tick().await;

            let current_iterations = ctx.iterations_count.load(Ordering::Relaxed);
            let diff_iterations = current_iterations - prev_iterations;

            let duration_in_seconds = ASK_MINING_PARAMS_PERIOD.as_secs_f64();
            let iterations_per_second = diff_iterations as f64 / duration_in_seconds;

            // Update exponential moving average
            ema_iterations_per_second = alpha * iterations_per_second + (1.0 - alpha) * ema_iterations_per_second;

            println!("⛏️  Speed: {:.2?} obj/s", ema_iterations_per_second);

            prev_iterations = current_iterations;

            let res = ctx.ask_mining_params().await;
            if let Err(e) = res {
                println!("🟥 Ask for mining params error: {}", &e);
            }
        }
    });
}

pub fn create_mining_obj() -> Vec<u8> {
    let radius: f32 = 1.0;

    let dents_count = 6;
    let dent_size: f32 = 0.4;

    let object = SphereUv::new(16, 12);

    let mut vertices: Vec<Vector3<f32>> = object.shared_vertex_iter()
        .map(|v| v.pos.into())
        .map(|v: [f32; 3]| Vector3::new(v[0], v[1], v[2]))
        .collect();

    // Move random N points towards the sphere center
    let mut rng = thread_rng();
    let vertices_count = vertices.len();
    for _ in 0..dents_count {
        let index = rng.gen_range(0, vertices_count);
        let distance = rng.gen_range(0.0, dent_size);
        let lerp_factor = distance / radius;
        vertices[index] = vertices[index].lerp(Vector3::new(0.0, 0.0, vertices[index].z), lerp_factor);
    }

    let scale_matrix = Matrix4::from_nonuniform_scale(0.8, 0.8, 1.0);

    vertices = vertices.into_iter()
        .map(|v| {
            let v4 = Vector4::new(v.x, v.y, v.z, 1.0); // Convert to Vector4
            let transformed_v4 = scale_matrix * v4;
            Vector3::new(transformed_v4.x, transformed_v4.y, transformed_v4.z) // Convert back to Vector3
        })
        .collect();

    let triangles: Vec<Triangle<usize>> = object.indexed_polygon_iter()
        .triangulate()
        .collect();

    let mut obj_data = String::new();

    // Add object name
    obj_data.push_str("o\n");

    // Add vertices
    for vertex in vertices.iter() {
        let pos = vertex * radius;
        obj_data.push_str(&format!("v {:.6} {:.6} {:.6}\n", pos.x, pos.y, pos.z));
    }

    // Add vertex normals (same as vertex positions, since it's a sphere)
    for vertex in vertices.iter() {
        let normal = vertex.normalize();
        obj_data.push_str(&format!("vn {:.6} {:.6} {:.6}\n", normal.x, normal.y, normal.z));
    }

    // Add faces
    for triangle in triangles.iter() {
        let f = triangle.map_vertex(|i| i + 1);
        obj_data.push_str(&format!("f {}//{} {}//{} {}//{}\n", f.x, f.x, f.y, f.y, f.z, f.z));
    }

    // println!("{}", obj_data);

    obj_data.into_bytes()
}
