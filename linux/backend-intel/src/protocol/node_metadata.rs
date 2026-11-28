//! The node metadata query, KMTQAITYPE_NODEMETADATA: what engine a node is, its name, and whether
//! it uses the GPU's own page tables. Direct3D asks it for each node it makes a context on, and
//! notes a failure when there is no answer.
//!
//! The answer is D3DKMT_NODEMETADATA: the node's ordinal as asked, then DXGK_NODEMETADATA's engine
//! type, a name of [`NAME_CHARS`] UTF-16 units, its flags, and GpuMmuSupported and IoMmuSupported.

use super::bytes::dword;
use super::nodes;
use guesthouse_backend::{Status, VendorQuery};

/// KMTQAITYPE_NODEMETADATA.
pub const QUERY_NODE_METADATA: u32 = 25;
const ANSWER_SIZE: usize = 78;
const NAME_CHARS: usize = 32;

/// DXGK_ENGINE_TYPE.
const ENGINE_OTHER: u32 = 0;
const ENGINE_3D: u32 = 1;
const ENGINE_VIDEO_DECODE: u32 = 2;
const ENGINE_VIDEO_PROCESSING: u32 = 4;
const ENGINE_COPY: u32 = 6;

/// GPUNODE_ORDINAL's nodes with an engine type of their own.
const NODE_3D: u32 = 0;
const NODE_VIDEO: u32 = 1;
const NODE_BLITTER: u32 = 2;
const NODE_VIDEO_ENHANCE: u32 = 3;
const NODE_VIDEO_2: u32 = 4;

/// Every node uses the GPU's page tables, as the record's FtrWddm2GpuMmu says, and none the IOMMU.
const GPU_MMU_SUPPORTED: u8 = 1;
const IO_MMU_SUPPORTED: u8 = 0;

/// The engine type of a node. Compute has none in DXGK_ENGINE_TYPE, so it is "other".
fn engine(node: u32) -> u32 {
    match node {
        NODE_3D => ENGINE_3D,
        NODE_VIDEO | NODE_VIDEO_2 => ENGINE_VIDEO_DECODE,
        NODE_BLITTER => ENGINE_COPY,
        NODE_VIDEO_ENHANCE => ENGINE_VIDEO_PROCESSING,
        _ => ENGINE_OTHER,
    }
}

/// Answers for a node that is offered. Anything else is left to the core.
pub fn answer(query: &VendorQuery<'_>, out: &mut Vec<u8>) -> Option<Status> {
    if query.kind != QUERY_NODE_METADATA || query.size != ANSWER_SIZE {
        return None;
    }
    let asked = dword(query.data, 0)?;
    // The low half is the node's ordinal and the high half the adapter's index.
    let node = asked & 0xFFFF;
    if !nodes::offered(node) {
        return None;
    }
    let start = out.len();
    out.extend_from_slice(&asked.to_le_bytes());
    out.extend_from_slice(&engine(node).to_le_bytes());
    nodes::name(node)
        .encode_utf16()
        .take(NAME_CHARS)
        .for_each(|unit| out.extend_from_slice(&unit.to_le_bytes()));
    out.resize(start + 8 + NAME_CHARS * 2, 0);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&[GPU_MMU_SUPPORTED, IO_MMU_SUPPORTED]);
    Some(Status::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(node: u32) -> [u8; ANSWER_SIZE] {
        let mut data = [0; ANSWER_SIZE];
        data[..4].copy_from_slice(&node.to_le_bytes());
        data
    }

    fn query(data: &[u8]) -> VendorQuery<'_> {
        VendorQuery {
            kind: QUERY_NODE_METADATA,
            size: ANSWER_SIZE,
            data,
            process_is_32_bit: false,
            program: "dwm.exe",
        }
    }

    #[test]
    fn a_node_is_answered_with_its_engine_name_and_gpu_page_tables() {
        let data = asked(0);
        let mut out = Vec::new();
        assert_eq!(answer(&query(&data), &mut out), Some(Status::SUCCESS));
        assert_eq!(out.len(), ANSWER_SIZE);
        assert_eq!(dword(&out, 4), Some(ENGINE_3D));
        assert_eq!(&out[8..14], &[b'3', 0, b'd', 0, 0, 0]);
        assert_eq!(&out[76..], &[1, 0]);
    }

    #[test]
    fn each_kind_of_node_has_its_engine_type() {
        assert_eq!(
            [engine(0), engine(2), engine(5), engine(3)],
            [ENGINE_3D, ENGINE_COPY, ENGINE_OTHER, ENGINE_VIDEO_PROCESSING]
        );
    }

    #[test]
    fn a_node_past_those_offered_or_a_wrong_size_is_left_to_the_core() {
        let data = asked(nodes::COUNT);
        assert_eq!(answer(&query(&data), &mut Vec::new()), None);
        let first = asked(0);
        let short = VendorQuery { size: 40, ..query(&first) };
        assert_eq!(answer(&short, &mut Vec::new()), None);
    }
}
