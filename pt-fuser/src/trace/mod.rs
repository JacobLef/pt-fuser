pub mod builder;
pub mod metrics;
pub mod trace_error;

#[cfg(test)]
mod test;

use std::{fmt::Display, io::Read};

use flate2::Compression;
use flexbuffers::FlexbufferSerializer;
use serde::{Deserialize, Serialize};

use crate::trace::metrics::{Metrics, MetricsRange};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

impl SymbolInfo {
    pub fn contains(&self, addr: u64) -> bool {
        self.offset <= addr && addr < self.offset + self.size
    }
}

impl Display for SymbolInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[0x{:x} - 0x{:x}] {}",
            self.offset,
            self.offset + self.size,
            self.name
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Straightline {
    pub metrics: MetricsRange,
}

pub trait Frame: Display {
    type ChildFrame: Frame;

    fn metrics(&self) -> &MetricsRange;
    fn metrics_mut(&mut self) -> &mut MetricsRange;
    fn add_child(&mut self, child: Self::ChildFrame) -> Result<(), Error>;
    fn chunks(&self) -> &[Chunk<Self::ChildFrame>];
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RootFrame<F: Frame> {
    metrics: MetricsRange,
    // INVARIANT: sum of time, cycles, and insn across all children must equal this frame's time, cycles, and insn
    chunks: Vec<Chunk<F>>,
}

impl<F: Frame> Display for RootFrame<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Unnamed Frame ({} - {})",
            self.metrics.start, self.metrics.end
        )
    }
}

impl<F: Frame> Frame for RootFrame<F> {
    type ChildFrame = F;

    #[inline]
    fn metrics(&self) -> &MetricsRange {
        &self.metrics
    }

    #[inline]
    fn metrics_mut(&mut self) -> &mut MetricsRange {
        &mut self.metrics
    }

    fn add_child(&mut self, child: Self::ChildFrame) -> Result<(), Error> {
        for mut i in 0..self.chunks.len() {
            match self.chunks[i] {
                Chunk::Frame(_) => continue,
                Chunk::Straightline(straightline) => {
                    if straightline.metrics.includes_range(&child.metrics()) {
                        if child.metrics().start != straightline.metrics.start {
                            let before = Straightline {
                                metrics: MetricsRange::new(
                                    straightline.metrics.start,
                                    child.metrics().start,
                                ),
                            };
                            self.chunks.insert(i, before.into());
                            i += 1;
                        }
                        if child.metrics().end != straightline.metrics.end {
                            let after = Straightline {
                                metrics: MetricsRange::new(
                                    child.metrics().end,
                                    straightline.metrics.end,
                                ),
                            };
                            self.chunks.insert(i + 1, after.into());
                        }
                        self.chunks[i] = child.into();
                        return Ok(());
                    }
                }
            }
        }

        if child.metrics().total_time() == 0
            && child.metrics().total_cycles() == 0
            && child.metrics().total_insn() == 0
        {
            if child.metrics().start == self.metrics.start {
                self.chunks.insert(0, child.into());
                return Ok(());
            } else if child.metrics().end == self.metrics.end {
                self.chunks.push(child.into());
                return Ok(());
            }
        }

        Err(Error::InvalidRange(child.metrics().clone()))
    }

    fn chunks(&self) -> &[Chunk<Self::ChildFrame>] {
        &self.chunks
    }
}

impl<F: Frame> RootFrame<F> {
    pub fn new(metrics: MetricsRange) -> Self {
        Self {
            metrics,
            chunks: vec![Chunk::Straightline(Straightline { metrics })],
        }
    }

    pub fn check_invariant(&self) -> bool {
        let mut total_time = 0;
        let mut total_cycles = 0;
        let mut total_insn = 0;
        for chunk in self.chunks() {
            total_time += chunk.total_time();
            total_cycles += chunk.total_cycles();
            total_insn += chunk.total_insn();
        }

        total_time == self.metrics().total_time()
            && total_cycles == self.metrics().total_cycles()
            && total_insn == self.metrics().total_insn()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NamedFrame {
    pub symbol: SymbolInfo,
    inner: RootFrame<NamedFrame>,
}

impl Display for NamedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} - {})",
            self.symbol,
            self.metrics().start,
            self.metrics().end
        )
    }
}

impl Frame for NamedFrame {
    type ChildFrame = NamedFrame;

    #[inline]
    fn metrics(&self) -> &MetricsRange {
        self.inner.metrics()
    }

    #[inline]
    fn metrics_mut(&mut self) -> &mut MetricsRange {
        self.inner.metrics_mut()
    }

    #[inline]
    fn add_child(&mut self, child: Self::ChildFrame) -> Result<(), Error> {
        self.inner.add_child(child)
    }

    #[inline]
    fn chunks(&self) -> &[Chunk<Self::ChildFrame>] {
        &self.inner.chunks()
    }
}

impl NamedFrame {
    pub fn new(metrics: MetricsRange, symbol: SymbolInfo) -> Self {
        Self {
            symbol,
            inner: RootFrame::new(metrics),
        }
    }

    pub fn check_invariant(&self) -> bool {
        self.inner.check_invariant()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chunk<F: Frame> {
    Frame(F),
    Straightline(Straightline),
}

impl<F: Frame> Chunk<F> {
    pub fn total_time(&self) -> u64 {
        match self {
            Chunk::Frame(frame) => frame.metrics().total_time(),
            Chunk::Straightline(straightline) => straightline.metrics.total_time(),
        }
    }

    pub fn total_cycles(&self) -> u64 {
        match self {
            Chunk::Frame(frame) => frame.metrics().total_cycles(),
            Chunk::Straightline(straightline) => straightline.metrics.total_cycles(),
        }
    }

    pub fn total_insn(&self) -> u64 {
        match self {
            Chunk::Frame(frame) => frame.metrics().total_insn(),
            Chunk::Straightline(straightline) => straightline.metrics.total_insn(),
        }
    }
}

impl<F: Frame> From<Straightline> for Chunk<F> {
    fn from(straightline: Straightline) -> Self {
        Chunk::Straightline(straightline)
    }
}

impl<F: Frame> From<F> for Chunk<F> {
    fn from(frame: F) -> Self {
        Chunk::Frame(frame)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Event {
    pub id: u32,
    // INVARIANT: occurences must be sorted by timestamp
    occurences: Vec<Metrics>,
    pub name: String,
    pub description: String,
}

impl Event {
    pub fn new(id: u32, name: String, description: String) -> Self {
        Self {
            id,
            occurences: Vec::new(),
            name,
            description,
        }
    }

    pub fn from_occurences(
        id: u32,
        name: String,
        description: String,
        occurences: Vec<Metrics>,
    ) -> Result<Self, Error> {
        if !occurences.is_sorted() {
            return Err(Error::NotSorted);
        }
        Ok(Self {
            id,
            occurences,
            name,
            description,
        })
    }

    pub fn add_occurence(&mut self, occurence: Metrics) {
        let idx = self.occurences.partition_point(|&x| x <= occurence);
        self.occurences.insert(idx, occurence);
    }

    pub fn occurences(&self) -> &[Metrics] {
        &self.occurences
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Trace {
    root: RootFrame<NamedFrame>,
    events: Vec<Event>,
}

impl Trace {
    pub fn new(root: RootFrame<NamedFrame>, events: Vec<Event>) -> Self {
        Self { root, events }
    }

    pub fn root_frame(&self) -> &RootFrame<NamedFrame> {
        &self.root
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn get_event(&self, id: u32) -> Option<&Event> {
        self.events.iter().find(|event| event.id == id)
    }

    pub fn bin_serialize(&self, gzip: bool) -> Result<Vec<u8>, flexbuffers::SerializationError> {
        let mut serializer = FlexbufferSerializer::new();
        self.serialize(&mut serializer)?;
        if gzip {
            let encoded = serializer.take_buffer();
            let mut encoder = flate2::read::GzEncoder::new(&encoded[..], Compression::default());
            let mut result = Vec::new();
            encoder.read_to_end(&mut result).unwrap();
            Ok(result)
        } else {
            Ok(serializer.take_buffer())
        }
    }

    pub fn bin_deserialize(
        data: &[u8],
        gzip: bool,
    ) -> Result<Self, flexbuffers::DeserializationError> {
        let decoded_data = if gzip {
            let mut decoder = flate2::read::GzDecoder::new(data);
            let mut decoded = Vec::new();
            decoder.read_to_end(&mut decoded).unwrap();
            decoded
        } else {
            data.to_vec()
        };
        flexbuffers::from_slice(&decoded_data)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Error {
    InvalidRange(MetricsRange),
    NotSorted,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidRange(range) => write!(f, "Invalid range: {}", range),
            Error::NotSorted => write!(f, "Occurences are not sorted by timestamp"),
        }
    }
}
