//! virtio-fs device: one shared host directory, served by [`crate::fuse::Share`].
//!
//! https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html#x1-49600011

use crate::{
    fuse::Share,
    virtio::{
        common,
        device::{Device, DeviceContext},
    },
};
use zerocopy::{FromBytes, Immutable, IntoBytes};

const DEVICE_ID: u32 = 26;
/// One high-priority queue (FORGET) and one request queue.
const NUM_QUEUES: u16 = 2;
const TAG_LEN: usize = 36;

#[derive(IntoBytes, FromBytes, Immutable)]
#[repr(C, packed)]
struct FsConfig {
    tag: [u8; TAG_LEN],
    num_request_queues: u32,
}

pub struct Fs {
    config: FsConfig,
    share: Share,
}

impl Fs {
    pub fn new(tag: &str, share: Share) -> Self {
        assert!(tag.len() < TAG_LEN, "virtio-fs tag {tag:?} is too long");
        let mut config = FsConfig {
            tag: [0; TAG_LEN],
            num_request_queues: u32::from(NUM_QUEUES) - 1,
        };
        config.tag[..tag.len()].copy_from_slice(tag.as_bytes());
        Self { config, share }
    }
}

impl Device for Fs {
    fn id(&self) -> u32 {
        DEVICE_ID
    }

    fn features(&self) -> u64 {
        common::feature::VERSION_1
    }

    fn num_queues(&self) -> u16 {
        NUM_QUEUES
    }

    fn read_config(&self, offset: u64, data: &mut [u8]) {
        let bytes = self.config.as_bytes();
        let offset = offset as usize;
        let end = (offset + data.len()).min(bytes.len());
        if offset < end {
            data[..end - offset].copy_from_slice(&bytes[offset..end]);
        }
    }

    fn queue_notified(&mut self, queue_idx: usize, ctx: &mut DeviceContext<'_>) {
        while let Some(chain) = ctx.pop_chain(queue_idx) {
            let mut request = vec![0u8; chain.data.readable_len()];
            if chain.data.read_at(0, &mut request, ctx.mem()).is_none() {
                eprintln!("virtio-fs: failed to read request from guest memory");
                ctx.complete(chain.token, 0);
                continue;
            }
            let written = match self.share.handle(&request) {
                Some(reply) => chain.data.write_response(&reply, ctx.mem()),
                None => 0,
            };
            ctx.complete(chain.token, written);
        }
    }
}
