use std::convert::TryInto;
use std::f32::consts::PI;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use cgmath::{InnerSpace, Matrix4, Rad, Vector3, Vector4, VectorSpace};
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

const ASK_MINING_PARAMS_PERIOD: Duration = Duration::from_secs(3);

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

pub fn get_hash_difficulty(hash: &H256) -> U256 {
    let num_hash = U256::from(&hash[..]);
    let max = U256::max_value();
    max / num_hash
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
        if first_hash == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" {
            ctx.bad_objects.fetch_add(1, Ordering::Relaxed);
            // println!("🟥 Zero-contour object found");
            // DEBUG: write mining_obj.obj to file in /tmp/objects/{random_name}.obj
            // use std::fs::File;
            // use std::io::Write;
            // let mut file = File::create(format!("/tmp/objects/{}.obj", rand::thread_rng().gen::<u32>())).unwrap();
            // file.write_all(mining_obj.obj.as_slice()).unwrap();
        }

        let mut lock = ctx.seen_objects.lock().unwrap();
        if !(*lock).insert(obj_hash.clone()) {
            ctx.dupe_objects.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        let diff = get_hash_difficulty(&comp.get_work());

        if diff >= pow_dfclty {
            let prop = MiningProposal {
                params: mining_params.clone(),
                hash: obj_hash,
                obj_id: mining_obj.obj_id,
                obj: mining_obj.obj.clone(),
            };
            ctx.push_to_queue(prop);
            println!("💎 Hash meets pow difficulty: {} > {}", &diff, &pow_dfclty);
        }

        if diff >= win_dfclty {
            let prop = MiningProposal {
                params: mining_params.clone(),
                hash: obj_hash,
                obj_id: mining_obj.obj_id,
                obj: mining_obj.obj,
            };
            ctx.push_to_queue(prop);
            println!("🔥💎 Hash meets win difficulty: {} > {}", &diff, &win_dfclty);
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
        let mut prev_bad_objects: usize = 0;
        let mut ema_bad_objects_per_second: f64 = 0.0;
        let mut prev_dupe_objects: usize = 0;
        let mut ema_dupe_objects_per_second: f64 = 0.0;
        // EMA smoothing factor between 0 and 1; higher value means more smoothing
        let alpha: f64 = 0.8;

        loop {
            interval.tick().await;

            let current_iterations = ctx.iterations_count.load(Ordering::Relaxed);
            let diff_iterations = current_iterations - prev_iterations;

            let current_bad_objects = ctx.bad_objects.load(Ordering::Relaxed);
            let diff_bad_objects = current_bad_objects - prev_bad_objects;

            let current_dupe_objects = ctx.dupe_objects.load(Ordering::Relaxed);
            let diff_dupe_objects = current_dupe_objects - prev_dupe_objects;

            let duration_in_seconds = ASK_MINING_PARAMS_PERIOD.as_secs_f64();
            let iterations_per_second = diff_iterations as f64 / duration_in_seconds;
            let bad_objects_per_second = diff_bad_objects as f64 / duration_in_seconds;
            let dupe_objects_per_second = diff_dupe_objects as f64 / duration_in_seconds;

            // Update exponential moving average
            ema_iterations_per_second = alpha * iterations_per_second + (1.0 - alpha) * ema_iterations_per_second;
            ema_bad_objects_per_second = alpha * bad_objects_per_second + (1.0 - alpha) * ema_bad_objects_per_second;
            ema_dupe_objects_per_second = alpha * dupe_objects_per_second + (1.0 - alpha) * ema_dupe_objects_per_second;

            if ema_iterations_per_second > 0.0 {
                // println a table with speed and bad objects percentage
                println!(
                    "⏱️  Speed: {:.2} it/s, {:.2}% bad objects, {:.2}% dupe objects",
                    ema_iterations_per_second,
                    ema_bad_objects_per_second / ema_iterations_per_second * 100.0,
                    ema_dupe_objects_per_second / ema_iterations_per_second * 100.0,
                );
            }

            prev_iterations = current_iterations;
            prev_bad_objects = current_bad_objects;
            prev_dupe_objects = current_dupe_objects;

            let res = ctx.ask_mining_params().await;
            if let Err(e) = res {
                println!("🟥 Ask for mining params error: {}", &e);
            }
        }
    });
}

pub fn create_mining_obj() -> Vec<u8> {
    let dents_count = 36;
    let dent_size: f32 = 0.2;

    let object = SphereUv::new(15, 13);

    let mut vertices: Vec<Vector3<f32>> = object.shared_vertex_iter()
        .map(|v| v.pos.into())
        .map(|v: [f32; 3]| Vector3::new(v[0], v[1], v[2]))
        .collect();

    let mut rng = thread_rng();
    let vertices_count = vertices.len();
    for _ in 0..dents_count {
        let index = rng.gen_range(0, vertices_count);
        let distance = rng.gen_range(0.0, dent_size);
        vertices[index] = vertices[index].lerp(Vector3::new(0.0, 0.0, 0.0), distance);
    }

    let transformation_matrix = Matrix4::from_nonuniform_scale(0.8, 0.8, 1.0) *
        Matrix4::from_angle_x(Rad(PI / 2.0));

    vertices.iter_mut()
        .for_each(|v| {
            let v4 = Vector4::new(v.x, v.y, v.z, 1.0); // Convert to Vector4
            let transformed_v4 = transformation_matrix * v4;
            *v = Vector3::new(transformed_v4.x, transformed_v4.y, transformed_v4.z);
        });

    let triangles: Vec<Triangle<usize>> = object.indexed_polygon_iter()
        .triangulate()
        .collect();

    let mut obj_data = String::with_capacity(vertices_count * 54);

    obj_data.push_str("o\n");

    for vertex in vertices.iter() {
        obj_data.push_str(&format!("v {:.6} {:.6} {:.6}\n", vertex.x, vertex.y, vertex.z));
    }

    for vertex in vertices.iter() {
        let normal = vertex.normalize();
        obj_data.push_str(&format!("vn {:.6} {:.6} {:.6}\n", normal.x, normal.y, normal.z));
    }

    for triangle in triangles.iter() {
        let f = triangle.map_vertex(|i| i + 1);
        obj_data.push_str(&format!("f {}//{} {}//{} {}//{}\n", f.x, f.x, f.y, f.y, f.z, f.z));
    }

    obj_data.into_bytes()
}
