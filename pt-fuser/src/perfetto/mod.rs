use std::collections::HashMap;

use perfetto_rust::{
    EventName, InternedData, TracePacket, TrackDescriptor, TrackEvent,
    trace_packet::{Data, OptionalTrustedPacketSequenceId, SequenceFlags},
    track_descriptor::{ChildTracksOrdering, StaticOrDynamicName},
    track_event::{self, NameField},
};
use prost::Message;

use crate::trace::{Chunk, Frame, Trace};

const GLOBAL_TRACK_ID: u64 = 10;
const GLOBAL_SEQUENCE_ID: u32 = 1;
const GLOBAL_TRACK_NAME: &str = "Process";

// These IDs are arbitrary but must be used consistently
const TRACE_TRACK_ID: u64 = 20;
const TRACE_SEQUENCE_ID: u32 = 2;
const TRACE_TRACK_NAME: &str = "Trace";

const ERROR_TRACK_ID_BASE: u64 = 30;
const ERROR_SEQUENCE_ID_BASE: u32 = 3;

fn create_track(
    timestamp: u64,
    sequence_id: u32,
    uuid: u64,
    name: String,
    desc: Option<String>,
    parent_uuid: Option<u64>,
    child_ordering: Option<ChildTracksOrdering>,
    sibling_order_rank: Option<i32>,
) -> TracePacket {
    let mut track = TracePacket::default();
    track.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(sequence_id),
    );
    track.sequence_flags = Some(SequenceFlags::SeqIncrementalStateCleared as u32);
    track.previous_packet_dropped = Some(true);
    track.first_packet_on_sequence = Some(true);
    track.timestamp = Some(timestamp);

    let mut description = TrackDescriptor::default();
    description.parent_uuid = parent_uuid;
    description.uuid = Some(uuid);
    description.static_or_dynamic_name = Some(StaticOrDynamicName::StaticName(name));
    description.description = desc;
    description.child_ordering = child_ordering.map(|x| x as i32);
    description.sibling_order_rank = sibling_order_rank;
    track.data = Some(Data::TrackDescriptor(description));

    track
}

fn create_slice_begin(
    timestamp: u64,
    sequence_id: u32,
    track_id: u64,
    name: NameField,
) -> TracePacket {
    let mut slice_begin = TracePacket::default();
    slice_begin.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(sequence_id),
    );
    slice_begin.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    slice_begin.timestamp = Some(timestamp);

    let mut slice_begin_event = TrackEvent::default();
    slice_begin_event.r#type = Some(track_event::Type::SliceBegin as i32);
    slice_begin_event.track_uuid = Some(track_id);
    slice_begin_event.name_field = Some(name);

    slice_begin.data = Some(Data::TrackEvent(slice_begin_event));

    slice_begin
}

fn create_slice_end(timestamp: u64, sequence_id: u32, track_id: u64) -> TracePacket {
    let mut slice_end = TracePacket::default();
    slice_end.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(sequence_id),
    );
    slice_end.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    slice_end.timestamp = Some(timestamp);

    let mut slice_end_event = TrackEvent::default();
    slice_end_event.r#type = Some(track_event::Type::SliceEnd as i32);
    slice_end_event.track_uuid = Some(track_id);

    slice_end.data = Some(Data::TrackEvent(slice_end_event));

    slice_end
}

fn create_event(timestamp: u64, event_id: u32) -> TracePacket {
    let mut event = TracePacket::default();
    event.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(ERROR_SEQUENCE_ID_BASE + event_id),
    );
    event.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    event.timestamp = Some(timestamp);

    let mut instant_event = TrackEvent::default();
    instant_event.r#type = Some(track_event::Type::Instant as i32);
    instant_event.track_uuid = Some(ERROR_TRACK_ID_BASE + event_id as u64);
    instant_event.name_field = Some(NameField::NameIid(1));

    event.data = Some(Data::TrackEvent(instant_event));

    event
}

struct Converter {
    interned_names: HashMap<String, u64>,
    last_iid: u64,
}

impl Converter {
    fn new() -> Self {
        Self {
            interned_names: HashMap::new(),
            last_iid: 0,
        }
    }

    fn process_frame(&mut self, frame: &Frame, stack_iid: &mut Vec<u64>) -> Vec<TracePacket> {
        let mut packets = Vec::new();

        let mut intern_data = None;
        let iid = self
            .interned_names
            .get(&frame.symbol.name)
            .copied()
            .unwrap_or_else(|| {
                self.last_iid += 1;
                self.interned_names
                    .insert(frame.symbol.name.clone(), self.last_iid);

                let mut new_intern_data = InternedData::default();
                new_intern_data.event_names = vec![EventName {
                    iid: Some(self.last_iid),
                    name: Some(frame.symbol.name.clone()),
                }];
                intern_data = Some(new_intern_data);

                self.last_iid
            });

        let mut slice_begin = create_slice_begin(
            frame.metrics.start.ts,
            TRACE_SEQUENCE_ID,
            TRACE_TRACK_ID,
            NameField::NameIid(iid),
        );
        slice_begin.interned_data = intern_data;
        packets.push(slice_begin);

        stack_iid.push(iid);

        for chunk in frame.chunks() {
            match chunk {
                Chunk::Frame(child) => packets.extend(self.process_frame(child, stack_iid)),
                Chunk::Straightline(_) => continue,
                Chunk::Pause(metrics) => {
                    // pretend all previous stack frames end here
                    for _ in 0..stack_iid.len() {
                        let slice_end =
                            create_slice_end(metrics.start.ts, TRACE_SEQUENCE_ID, TRACE_TRACK_ID);
                        packets.push(slice_end);
                    }

                    // previous stack frames resume once pause is over
                    // in Perfetto, this appears as a blank gap, indicating that tracing was paused
                    let resume = metrics.end.ts;
                    for iid in stack_iid.iter() {
                        let slice_begin = create_slice_begin(
                            resume,
                            TRACE_SEQUENCE_ID,
                            TRACE_TRACK_ID,
                            NameField::NameIid(*iid),
                        );
                        packets.push(slice_begin);
                    }
                }
            }
        }

        stack_iid.pop();

        let slice_end = create_slice_end(frame.metrics.end.ts, TRACE_SEQUENCE_ID, TRACE_TRACK_ID);
        packets.push(slice_end);

        packets
    }
}

pub fn convert_to_perfetto(trace: &Trace) -> Vec<u8> {
    let mut converter = Converter::new();

    let mut packets = Vec::new();
    packets.push(create_track(
        trace.root_frame().metrics.start.ts,
        GLOBAL_SEQUENCE_ID,
        GLOBAL_TRACK_ID,
        GLOBAL_TRACK_NAME.to_string(),
        None,
        None,
        Some(ChildTracksOrdering::Explicit),
        None,
    ));
    packets.push(create_slice_begin(
        trace.root_frame().metrics.start.ts,
        GLOBAL_SEQUENCE_ID,
        GLOBAL_TRACK_ID,
        NameField::Name("Overall Latency".to_string()),
    ));
    packets.push(create_slice_end(
        trace.root_frame().metrics.end.ts,
        GLOBAL_SEQUENCE_ID,
        GLOBAL_TRACK_ID,
    ));

    packets.push(create_track(
        trace.root_frame().metrics.start.ts,
        TRACE_SEQUENCE_ID,
        TRACE_TRACK_ID,
        TRACE_TRACK_NAME.to_string(),
        None,
        Some(GLOBAL_TRACK_ID),
        None,
        Some(i32::MAX)
    ));
    packets.extend(converter.process_frame(trace.root_frame(), &mut Vec::new()));

    for event in trace.events() {
        if let Some(first_occurence) = event.occurences().first() {
            // Map 0..=u32::MAX to i32::MIN..=i32::MAX while preserving order
            let scaled_id = (event.id ^ 0x80000000) as i32;
            let mut event_start = create_track(
                first_occurence.ts,
                ERROR_SEQUENCE_ID_BASE + event.id,
                ERROR_TRACK_ID_BASE + event.id as u64,
                event.name.clone(),
                Some(event.description.clone()),
                Some(GLOBAL_TRACK_ID),
                None,
                Some(scaled_id),
            );

            let mut interned_data = InternedData::default();
            interned_data.event_names = vec![EventName {
                iid: Some(1),
                name: Some(event.description.clone()),
            }];
            event_start.interned_data = Some(interned_data);

            packets.push(event_start);

            for occurence in event.occurences() {
                let event_packet = create_event(occurence.ts, event.id);
                packets.push(event_packet);
            }
        }
    }

    let perfetto_trace = perfetto_rust::Trace { packet: packets };
    perfetto_trace.encode_to_vec()
}
