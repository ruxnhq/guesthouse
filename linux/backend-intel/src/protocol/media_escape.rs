//! The media driver's escape: what Intel's video decoder and video processor ask the kernel driver,
//! code [`MEDIA`], always [`SIZE`] bytes, with what it asks in the payload's first dword.
//!
//! Two of its questions decide whether video comes out at all. Before a decoder makes its context,
//! it asks which video decode engine to use and makes the context on the node it is told, and the
//! video enhancement engine is asked for the same way. The kernel driver hands a card's engines out
//! in turn and counts what each process holds, so that a stream ending gives its engine back. A
//! question answered by leaving it as it came says node 0, the 3D engine: the decoder then sends
//! its work where no video command does anything, its pictures stay empty, and NV12 of zeroes
//! shows as solid green. So these are answered as the kernel driver answers them, with the nodes
//! of the engines the card runs.
//!
//! The media driver also keeps a count of media resets with the kernel driver: it sets the count
//! when it makes a decoder or a video processor and reads it back when it destroys one. That is
//! kept here too. Every other question is answered as the kernel driver answers it, by leaving the
//! buffer as it came.

use super::bytes::{dword, put_dword};
use super::escape::HEADER_SIZE;
use std::collections::HashMap;
use std::fmt;

/// The media driver's escape code.
pub const MEDIA: u32 = 6;
/// The size of every media escape, header and payload, which is all the kernel driver takes.
pub const SIZE: usize = HEADER_SIZE + 0x160;

/// What is asked, the payload's first dword.
const ACTION: usize = HEADER_SIZE;
const SET_RESET_COUNT: u32 = 3;
const GET_RESET_COUNT: u32 = 4;
const VIDEO_DECODE_ENGINE: u32 = 0x10;
const VIDEO_ENHANCE_ENGINE: u32 = 0x18;

/// Where the reset count is, both ways.
const RESET_COUNT: usize = 0x84;
/// Where each engine question is: the request, then the node, a dword each.
const VIDEO_DECODE_QUESTION: usize = 0xC8;
const VIDEO_ENHANCE_QUESTION: usize = 0xD0;

/// The nodes the kernel driver hands out for each engine kind, first engine first. The first two
/// video decode nodes and the first video enhancement node are gmmlib's GPUNODE_VIDEO, GPUNODE_VCS2
/// and GPUNODE_VE; the kernel driver numbers a third video decode engine 8 and a second video
/// enhancement engine 9, which gmmlib gives other names.
const VIDEO_DECODE_NODES: [u32; 3] = [1, 4, 8];
const VIDEO_ENHANCE_NODES: [u32; 2] = [3, 9];

/// The two kinds of engine a media stream is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    VideoDecode,
    VideoEnhance,
}

impl EngineKind {
    fn word(self) -> &'static str {
        match self {
            Self::VideoDecode => "video-decode",
            Self::VideoEnhance => "video-enhance",
        }
    }
}

/// What a stream asks of its engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Request {
    /// It is given the engine whose turn it is.
    Assign,
    /// It gives back the engine it was given.
    Release,
    /// It takes an engine it names, as a second stream does to use the other engine.
    Take,
    /// Something the kernel driver refuses, which is left as it came.
    Unknown(u32),
}

impl Request {
    fn read(value: u32) -> Self {
        match value {
            0 => Self::Assign,
            1 => Self::Release,
            2 => Self::Take,
            other => Self::Unknown(other),
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Assign => "assign",
            Self::Release => "release",
            Self::Take => "take",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// The nodes of each kind whose engines the card runs, in the order the kernel driver hands them
/// out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaEngines {
    pub video_decode: Vec<u32>,
    pub video_enhance: Vec<u32>,
}

impl MediaEngines {
    /// The nodes of each kind that `runs` says the card has an engine for.
    pub fn of(runs: impl Fn(u32) -> bool) -> Self {
        let kept = |nodes: &[u32]| nodes.iter().copied().filter(|&node| runs(node)).collect();
        Self { video_decode: kept(&VIDEO_DECODE_NODES), video_enhance: kept(&VIDEO_ENHANCE_NODES) }
    }

    fn of_kind(&self, kind: EngineKind) -> &[u32] {
        match kind {
            EngineKind::VideoDecode => &self.video_decode,
            EngineKind::VideoEnhance => &self.video_enhance,
        }
    }
}

/// The nodes for the log, as `video_decode=1,4 video_enhance=3`, with `-` for a kind the card has
/// no engine of.
impl fmt::Display for MediaEngines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |nodes: &[u32]| match nodes {
            [] => "-".to_string(),
            nodes => nodes.iter().map(u32::to_string).collect::<Vec<_>>().join(","),
        };
        write!(f, "video_decode={} video_enhance={}", list(&self.video_decode), list(&self.video_enhance))
    }
}

/// What a media escape was answered with, for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Answered {
    /// An engine question: what was asked, the node the guest named, the node it ends up with, and
    /// how many streams the process now holds on that node.
    Engine { kind: EngineKind, request: Request, named: u32, node: u32, held: u32 },
    /// The reset count, set or read.
    ResetCount { set: bool, count: u32 },
}

impl fmt::Display for Answered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Engine { kind, request, named, node, held } => write!(
                f,
                "engine={} asked={} named={named} answered={node} held={held}",
                kind.word(),
                request.word()
            ),
            Self::ResetCount { set, count } => {
                write!(f, "reset-count {}={count}", if set { "set" } else { "read" })
            }
        }
    }
}

/// How many (process, node) pairs are kept in the count of streams held.
const MOST_HELD: usize = 4096;

/// What the kernel driver keeps for the media escapes: the reset count, and how many streams each
/// process holds on each engine node.
#[derive(Debug, Default)]
pub struct MediaEscapes {
    reset_count: u32,
    held: HashMap<(u32, u32), u32>,
}

impl MediaEscapes {
    /// Answers a media escape from `process`, if this is one and one of the questions kept here.
    pub fn answer(&mut self, process: u32, data: &mut [u8], engines: &MediaEngines) -> Option<Answered> {
        if data.len() != SIZE || dword(data, super::escape::CODE)? != MEDIA {
            return None;
        }
        match dword(data, ACTION)? {
            SET_RESET_COUNT => {
                self.reset_count = dword(data, RESET_COUNT)?;
                Some(Answered::ResetCount { set: true, count: self.reset_count })
            }
            GET_RESET_COUNT => {
                put_dword(data, RESET_COUNT, self.reset_count);
                Some(Answered::ResetCount { set: false, count: self.reset_count })
            }
            VIDEO_DECODE_ENGINE => {
                self.engine(process, data, VIDEO_DECODE_QUESTION, EngineKind::VideoDecode, engines)
            }
            VIDEO_ENHANCE_ENGINE => {
                self.engine(process, data, VIDEO_ENHANCE_QUESTION, EngineKind::VideoEnhance, engines)
            }
            _ => None,
        }
    }

    /// A process has ended, and the engines its streams held are free again.
    pub fn process_gone(&mut self, process: u32) {
        self.held.retain(|&(holder, _), _| holder != process);
    }

    /// The guest has restarted, and nothing it held is held any more.
    pub fn guest_restarted(&mut self) {
        self.held.clear();
        self.reset_count = 0;
    }

    fn engine(
        &mut self,
        process: u32,
        data: &mut [u8],
        at: usize,
        kind: EngineKind,
        engines: &MediaEngines,
    ) -> Option<Answered> {
        let request = Request::read(dword(data, at)?);
        let named = dword(data, at + 4)?;
        let nodes = engines.of_kind(kind);
        let node = match request {
            Request::Assign => self.hand_out(process, nodes, kind),
            // A node the card doesn't run would be refused, and the stream would have no engine; it
            // is given the one whose turn it is, which it reads back and uses.
            Request::Take if nodes.contains(&named) => {
                self.hold(process, named);
                named
            }
            Request::Take => self.hand_out(process, nodes, kind),
            Request::Release => {
                if let Some(count) = self.held.get_mut(&(process, named)) {
                    *count = count.saturating_sub(1);
                }
                named
            }
            Request::Unknown(_) => named,
        };
        put_dword(data, at + 4, node);
        let held = self.held.get(&(process, node)).copied().unwrap_or(0);
        Some(Answered::Engine { kind, request, named, node, held })
    }

    /// Counts one more stream of `process` on `node`. Counts stop at the most a u32 holds, and
    /// past [`MOST_HELD`] distinct holders a new one is not kept.
    fn hold(&mut self, process: u32, node: u32) {
        if self.held.len() >= MOST_HELD && !self.held.contains_key(&(process, node)) {
            return;
        }
        let count = self.held.entry((process, node)).or_default();
        *count = count.saturating_add(1);
    }

    /// The node whose turn it is among `nodes`, counted by the streams every process holds on them,
    /// now held once more by `process`. With no engine of the kind the first node is given, which is
    /// also what the media driver uses when the kernel driver can't answer.
    fn hand_out(&mut self, process: u32, nodes: &[u32], kind: EngineKind) -> u32 {
        let Some(&first) = nodes.first() else {
            return match kind {
                EngineKind::VideoDecode => VIDEO_DECODE_NODES[0],
                EngineKind::VideoEnhance => VIDEO_ENHANCE_NODES[0],
            };
        };
        let held = self
            .held
            .iter()
            .filter(|((_, node), _)| nodes.contains(node))
            .fold(0u32, |sum, (_, &count)| sum.saturating_add(count));
        let node = nodes.get(held as usize % nodes.len()).copied().unwrap_or(first);
        self.hold(process, node);
        node
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::escape::{self, Escape, Verdict};

    const EDGE: u32 = 0x4003_2A80;
    const PLAYER: u32 = 0x4004_B780;

    /// A media escape asking `action`, otherwise zero, as the media driver sends it.
    pub fn media(action: u32) -> Vec<u8> {
        let mut data = vec![0u8; SIZE];
        put_dword(&mut data, 0, (SIZE - HEADER_SIZE) as u32);
        put_dword(&mut data, escape::CODE, MEDIA);
        put_dword(&mut data, ACTION, action);
        data
    }

    /// An engine question: `request` for `node` of the decode engine, or of the enhancement engine.
    pub fn question(kind: EngineKind, request: u32, node: u32) -> Vec<u8> {
        let (action, at) = match kind {
            EngineKind::VideoDecode => (VIDEO_DECODE_ENGINE, VIDEO_DECODE_QUESTION),
            EngineKind::VideoEnhance => (VIDEO_ENHANCE_ENGINE, VIDEO_ENHANCE_QUESTION),
        };
        let mut data = media(action);
        put_dword(&mut data, at, request);
        put_dword(&mut data, at + 4, node);
        data
    }

    /// The engines of an Arc B580 as xe lists them: two video decode engines and two video
    /// enhancement engines, of which only the first has a node here.
    fn b580() -> MediaEngines {
        MediaEngines::of(|node| [1, 3, 4].contains(&node))
    }

    /// The video decode node a question's buffer names.
    pub fn video_decode_node(data: &[u8]) -> Option<u32> {
        dword(data, VIDEO_DECODE_QUESTION + 4)
    }

    fn ask(escapes: &mut MediaEscapes, process: u32, data: &mut [u8], engines: &MediaEngines) -> u32 {
        escapes.answer(process, data, engines).expect("a media escape kept here");
        video_decode_node(data).unwrap()
    }

    #[test]
    fn a_media_escape_is_taken_as_the_kernel_driver_takes_it() {
        let escape = Escape::parse(&media(VIDEO_DECODE_ENGINE));
        assert_eq!((escape.code, escape.length, escape.verdict), (MEDIA, SIZE, Verdict::Taken));
    }

    #[test]
    fn a_decoder_asking_for_an_engine_is_never_told_the_3d_node() {
        let mut escapes = MediaEscapes::default();
        let mut data = question(EngineKind::VideoDecode, 0, 0);
        let answered = escapes.answer(EDGE, &mut data, &b580());
        assert_eq!(dword(&data, VIDEO_DECODE_QUESTION + 4), Some(1), "the first video decode engine");
        assert_eq!(
            answered,
            Some(Answered::Engine {
                kind: EngineKind::VideoDecode,
                request: Request::Assign,
                named: 0,
                node: 1,
                held: 1
            })
        );
        assert_eq!(dword(&data, VIDEO_DECODE_QUESTION), Some(0), "the request is left as it came");
    }

    #[test]
    fn decoders_are_handed_the_engines_in_turn_and_given_back_ones_are_handed_again() {
        let mut escapes = MediaEscapes::default();
        let engines = b580();
        let mut assign = || ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &engines);
        assert_eq!([assign(), assign(), assign()], [1, 4, 1]);

        let mut escapes = MediaEscapes::default();
        assert_eq!(ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &engines), 1);
        ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 1, 1), &engines);
        assert_eq!(
            ask(&mut escapes, PLAYER, &mut question(EngineKind::VideoDecode, 0, 0), &engines),
            1,
            "the first engine was given back, so it is its turn again"
        );
    }

    #[test]
    fn a_stream_takes_the_engine_it_names_when_the_card_runs_it() {
        let mut escapes = MediaEscapes::default();
        let engines = b580();
        assert_eq!(ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 2, 4), &engines), 4);
        assert_eq!(
            ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &engines),
            4,
            "one stream holds the second engine, so the count says it is the second's turn"
        );
        assert_eq!(
            ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 2, 8), &engines),
            1,
            "the card has no third engine, so the stream is given the one whose turn it is"
        );
    }

    #[test]
    fn a_card_with_one_video_engine_hands_out_that_one() {
        let mut escapes = MediaEscapes::default();
        let one = MediaEngines::of(|node| node == 1 || node == 3);
        for _ in 0..3 {
            assert_eq!(ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &one), 1);
        }
        let none = MediaEngines::of(|_| false);
        assert_eq!(
            ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &none),
            1,
            "with no engine, what the media driver falls back to"
        );
    }

    #[test]
    fn the_engines_are_named_for_the_log() {
        assert_eq!(b580().to_string(), "video_decode=1,4 video_enhance=3");
        assert_eq!(MediaEngines::of(|_| false).to_string(), "video_decode=- video_enhance=-");
    }

    #[test]
    fn the_video_enhancement_engine_is_asked_for_at_its_own_place() {
        let mut escapes = MediaEscapes::default();
        let mut data = question(EngineKind::VideoEnhance, 0, 0);
        escapes.answer(EDGE, &mut data, &b580());
        assert_eq!(dword(&data, VIDEO_ENHANCE_QUESTION + 4), Some(3));
        assert_eq!(dword(&data, VIDEO_DECODE_QUESTION + 4), Some(0), "the decode question is untouched");
        let mut again = question(EngineKind::VideoEnhance, 0, 0);
        escapes.answer(PLAYER, &mut again, &b580());
        assert_eq!(dword(&again, VIDEO_ENHANCE_QUESTION + 4), Some(3), "the second has no node here");
    }

    #[test]
    fn a_gone_process_frees_what_it_held() {
        let mut escapes = MediaEscapes::default();
        let engines = b580();
        assert_eq!(ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &engines), 1);
        escapes.process_gone(EDGE);
        assert_eq!(ask(&mut escapes, PLAYER, &mut question(EngineKind::VideoDecode, 0, 0), &engines), 1);
        escapes.guest_restarted();
        assert_eq!(ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &engines), 1);
    }

    #[test]
    fn a_release_of_nothing_held_and_an_unknown_request_change_nothing() {
        let mut escapes = MediaEscapes::default();
        let engines = b580();
        let mut release = question(EngineKind::VideoDecode, 1, 4);
        escapes.answer(EDGE, &mut release, &engines);
        assert_eq!(release, question(EngineKind::VideoDecode, 1, 4));
        let mut unknown = question(EngineKind::VideoDecode, 7, 0);
        escapes.answer(EDGE, &mut unknown, &engines);
        assert_eq!(unknown, question(EngineKind::VideoDecode, 7, 0));
        assert_eq!(ask(&mut escapes, EDGE, &mut question(EngineKind::VideoDecode, 0, 0), &engines), 1);
    }

    #[test]
    fn counts_of_streams_stop_at_the_most_and_the_holders_are_few() {
        let mut escapes = MediaEscapes::default();
        let engines = b580();
        escapes.held.insert((EDGE, 1), u32::MAX);
        escapes.held.insert((PLAYER, 1), u32::MAX);
        for process in [EDGE, PLAYER] {
            ask(&mut escapes, process, &mut question(EngineKind::VideoDecode, 2, 1), &engines);
            ask(&mut escapes, process, &mut question(EngineKind::VideoDecode, 0, 0), &engines);
        }
        assert_eq!(escapes.held[&(EDGE, 1)], u32::MAX);
        for process in 0..MOST_HELD as u32 * 2 {
            ask(&mut escapes, process, &mut question(EngineKind::VideoDecode, 2, 1), &engines);
        }
        assert_eq!(escapes.held.len(), MOST_HELD);
    }

    #[test]
    fn the_reset_count_set_is_the_one_read_back() {
        let mut escapes = MediaEscapes::default();
        let engines = b580();
        let mut set = media(SET_RESET_COUNT);
        put_dword(&mut set, RESET_COUNT, 5);
        assert_eq!(
            escapes.answer(EDGE, &mut set, &engines),
            Some(Answered::ResetCount { set: true, count: 5 })
        );
        let mut read = media(GET_RESET_COUNT);
        put_dword(&mut read, RESET_COUNT, 0xFFFF);
        escapes.answer(EDGE, &mut read, &engines);
        assert_eq!(dword(&read, RESET_COUNT), Some(5));
    }

    #[test]
    fn other_actions_codes_and_sizes_are_left_as_they_came() {
        let mut escapes = MediaEscapes::default();
        let engines = b580();
        for action in [2, 8, 0x13, 0x15, 0x16] {
            let mut data = media(action);
            assert_eq!(escapes.answer(EDGE, &mut data, &engines), None, "{action:#x}");
            assert_eq!(data, media(action));
        }
        let mut other_code = question(EngineKind::VideoDecode, 0, 0);
        put_dword(&mut other_code, escape::CODE, 2);
        assert_eq!(escapes.answer(EDGE, &mut other_code, &engines), None);
        let mut short = question(EngineKind::VideoDecode, 0, 0);
        short.truncate(SIZE - 4);
        assert_eq!(escapes.answer(EDGE, &mut short, &engines), None);
    }
}
