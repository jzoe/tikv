// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

use std::time::Duration;
use std::thread;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tikv::storage::{Engine, Snapshot, Modify, ALL_CFS};
use tikv::storage::engine::{Callback, Result};
use tikv::storage::config::Config;
use kvproto::kvrpcpb::Context;
use raftstore::cluster::Cluster;
use raftstore::server::ServerCluster;
use raftstore::server::new_server_cluster_with_cfs;
use tikv::util::HandyRwLock;
use super::sync_storage::SyncStorage;

#[derive(Debug)]
pub struct BlockEngine {
    engine: Box<Engine>,
    block_write: Arc<AtomicBool>,
    block_snapshot: Arc<AtomicBool>,
}

impl BlockEngine {
    pub fn new(engine: Box<Engine>) -> BlockEngine {
        BlockEngine {
            engine: engine,
            block_write: Arc::new(AtomicBool::new(false)),
            block_snapshot: Arc::new(AtomicBool::new(false)),
        }
    }

    #[allow(dead_code)]
    pub fn block_write(&self) {
        self.block_write.store(true, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn unblock_write(&self) {
        self.block_write.store(false, Ordering::SeqCst);
    }

    pub fn block_snapshot(&self) {
        self.block_snapshot.store(true, Ordering::SeqCst);
    }

    pub fn unblock_snapshot(&self) {
        self.block_snapshot.store(false, Ordering::SeqCst);
    }
}

impl Engine for BlockEngine {
    fn async_write(&self, ctx: &Context, batch: Vec<Modify>, callback: Callback<()>) -> Result<()> {
        let block_write = self.block_write.clone();
        self.engine.async_write(ctx,
                                batch,
                                box move |res| {
            thread::spawn(move || {
                while block_write.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                }
                callback(res);
            });
        })
    }

    fn async_snapshot(&self, ctx: &Context, callback: Callback<Box<Snapshot>>) -> Result<()> {
        let block_snapshot = self.block_snapshot.clone();
        self.engine.async_snapshot(ctx,
                                   box move |res| {
            thread::spawn(move || {
                while block_snapshot.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                }
                callback(res);
            });
        })
    }

    fn clone(&self) -> Box<Engine + 'static> {
        box BlockEngine {
            engine: self.engine.clone(),
            block_write: self.block_write.clone(),
            block_snapshot: self.block_snapshot.clone(),
        }
    }
}

pub fn new_raft_engine(count: usize, key: &str) -> (Cluster<ServerCluster>, Box<Engine>, Context) {
    let mut cluster = new_server_cluster_with_cfs(0, count, ALL_CFS);
    cluster.run();
    // make sure leader has been elected.
    assert_eq!(cluster.must_get(b""), None);
    let region = cluster.get_region(key.as_bytes());
    let leader = cluster.leader_of_region(region.get_id()).unwrap();
    let engine = cluster.sim.rl().storages[&leader.get_id()].clone();
    let mut ctx = Context::new();
    ctx.set_region_id(region.get_id());
    ctx.set_region_epoch(region.get_region_epoch().clone());
    ctx.set_peer(leader.clone());
    (cluster, engine, ctx)
}

pub fn new_raft_storage_with_store_count(count: usize,
                                         key: &str)
                                         -> (Cluster<ServerCluster>, SyncStorage, Context) {
    let (cluster, engine, ctx) = new_raft_engine(count, key);
    (cluster, SyncStorage::from_engine(engine, &Config::default()), ctx)
}
