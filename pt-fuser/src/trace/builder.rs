use crate::trace::{
    self, Event, Frame, Metrics, MetricsRange, NamedFrame, RootFrame, SymbolInfo, Trace,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IncompleteFrame {
    start_metrics: Metrics,
    child_frames: Vec<NamedFrame>,
    symbol: SymbolInfo,
}

impl IncompleteFrame {
    fn complete(self, end_metrics: Metrics) -> Result<NamedFrame, trace::Error> {
        let mut completed = NamedFrame::new(
            MetricsRange::new(self.start_metrics, end_metrics),
            self.symbol,
        );
        for child in self.child_frames.into_iter().rev() {
            completed.add_child(child)?;
        }
        Ok(completed)
    }
}

#[derive(Debug)]
pub struct TraceBuilder {
    last_metrics: Metrics,
    current_frame: IncompleteFrame,
    callstack: Vec<IncompleteFrame>,
    events: Vec<Event>,
    prev_segments: Vec<NamedFrame>,
}

#[derive(Debug)]
pub struct PausedTraceBuilder {
    events: Vec<Event>,
    prev_segments: Vec<NamedFrame>,
    current_stack: Vec<SymbolInfo>,
}

impl TraceBuilder {
    fn ensure_monotonic(&self, new_metrics: Metrics) {
        if new_metrics.ts < self.last_metrics.ts
            || new_metrics.cycles < self.last_metrics.cycles
            || new_metrics.insn_count < self.last_metrics.insn_count
        {
            panic!(
                "Metrics must increase monotonically. Previous: {}, New: {}",
                self.last_metrics, new_metrics
            );
        }
    }

    pub fn new(start_metrics: Metrics, symbol: SymbolInfo) -> Self {
        Self::new_resume(start_metrics, vec![symbol], Vec::new(), Vec::new())
    }

    fn new_resume(
        start_metrics: Metrics,
        current_stack: Vec<SymbolInfo>,
        events: Vec<Event>,
        prev_segments: Vec<NamedFrame>,
    ) -> Self {
        let mut current_stack = current_stack
            .iter()
            .map(|sym| IncompleteFrame {
                start_metrics,
                child_frames: Vec::new(),
                symbol: sym.clone(),
            })
            .collect::<Vec<_>>();
        let current_frame = current_stack.pop().unwrap();
        Self {
            last_metrics: start_metrics,
            current_frame,
            callstack: current_stack,
            events,
            prev_segments,
        }
    }

    pub fn push_frame(&mut self, metrics: Metrics, symbol: SymbolInfo) {
        self.ensure_monotonic(metrics);
        let new_frame = IncompleteFrame {
            start_metrics: metrics,
            child_frames: Vec::new(),
            symbol,
        };
        let old_frame = std::mem::replace(&mut self.current_frame, new_frame);
        self.callstack.push(old_frame);
        self.last_metrics = metrics;
    }

    pub fn complete_frame(mut self, end_metrics: Metrics) -> Result<BuilderResult, trace::Error> {
        self.ensure_monotonic(end_metrics);
        if self.callstack.is_empty() {
            let completed_frame = self.current_frame.complete(end_metrics)?;

            let min_metric = self
                .prev_segments
                .first()
                .map_or(completed_frame.metrics().start, |f| f.metrics().start);
            let mut root_frame = RootFrame::new(MetricsRange::new(min_metric, end_metrics));
            for segment in self.prev_segments.into_iter() {
                root_frame.add_child(segment)?;
            }
            root_frame.add_child(completed_frame)?;

            Ok(BuilderResult::Completed(Trace::new(
                root_frame,
                self.events,
            )))
        } else {
            let prev = self.callstack.pop().unwrap();
            let current_frame = std::mem::replace(&mut self.current_frame, prev);
            let completed_frame = current_frame.complete(end_metrics)?;
            self.current_frame.child_frames.push(completed_frame);
            self.last_metrics = end_metrics;
            Ok(BuilderResult::Builder(self))
        }
    }

    pub fn new_event(&mut self, id: u32, name: String, description: String) {
        self.events.push(Event::new(id, name, description));
    }

    pub fn event_occured(&mut self, event_id: u32, metrics: Metrics) {
        for event in self.events.iter_mut() {
            if event.id == event_id {
                event.add_occurence(metrics);
            }
        }
    }

    pub fn callstack_depth(&self) -> usize {
        self.callstack.len() + 1
    }

    /// index = 0 means top of the callstack. Higher indices go down the callstack.
    pub fn get_frame_symbol(&self, index: usize) -> &SymbolInfo {
        if index == 0 {
            &self.current_frame.symbol
        } else {
            &self.callstack[self.callstack.len() - index].symbol
        }
    }

    pub fn pause(mut self, metrics: Metrics) -> Result<PausedTraceBuilder, trace::Error> {
        let mut current_stack = self
            .callstack
            .iter()
            .map(|frame| frame.symbol.clone())
            .collect::<Vec<_>>();
        current_stack.push(self.current_frame.symbol.clone());

        let mut curr_frame = self.current_frame;
        while let Some(mut prev) = self.callstack.pop() {
            let completed_frame = curr_frame.complete(metrics)?;
            prev.child_frames.push(completed_frame);
            curr_frame = prev;
        }

        self.prev_segments.push(curr_frame.complete(metrics)?);
        Ok(PausedTraceBuilder {
            events: self.events,
            prev_segments: self.prev_segments,
            current_stack,
        })
    }
}

impl PausedTraceBuilder {
    pub fn resume(self, start_metrics: Metrics) -> TraceBuilder {
        TraceBuilder::new_resume(
            start_metrics,
            self.current_stack,
            self.events,
            self.prev_segments,
        )
    }
}

pub enum BuilderResult {
    Builder(TraceBuilder),
    Completed(Trace),
}

#[cfg(test)]
mod test {
    use super::*;
    use trace::{
        Chunk,
        test::{INNER_RANGE1, INNER_RANGE2, METRICS_ONE, SAMPLE_RANGE, TEST_SYMBOL},
    };

    fn extract_builder(result: BuilderResult) -> TraceBuilder {
        match result {
            BuilderResult::Builder(builder) => builder,
            BuilderResult::Completed(_) => panic!("Expected builder, got completed trace"),
        }
    }

    fn extract_frame_chunk<F: Frame>(chunk: &Chunk<F>) -> &F {
        match chunk {
            Chunk::Frame(frame) => frame,
            _ => panic!("Expected frame chunk"),
        }
    }

    #[test]
    fn complete_empty_frame() {
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: Vec::new(),
            symbol: TEST_SYMBOL.clone(),
        };
        let completed = incomplete.complete(SAMPLE_RANGE.end).unwrap();
        assert_eq!(completed.chunks().len(), 1);
        assert!(completed.check_invariant());
    }

    #[test]
    fn complete_frame_with_chunks() {
        let inner1 = NamedFrame::new(INNER_RANGE1, TEST_SYMBOL.clone());
        let inner2 = NamedFrame::new(INNER_RANGE2, TEST_SYMBOL.clone());
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: vec![inner1, inner2],
            symbol: TEST_SYMBOL.clone(),
        };
        let completed = incomplete.complete(SAMPLE_RANGE.end).unwrap();
        assert_eq!(completed.chunks().len(), 5);
        assert!(completed.check_invariant());
    }

    #[test]
    fn build_trace_simple() {
        let builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        let result = builder.complete_frame(SAMPLE_RANGE.end).unwrap();
        match result {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root.chunks().len(), 1);
                assert_eq!(
                    trace.root.chunks()[0].total_time(),
                    SAMPLE_RANGE.total_time()
                );
                match &trace.root.chunks()[0] {
                    trace::Chunk::Frame(frame) => {
                        assert_eq!(frame.metrics(), &SAMPLE_RANGE);
                        assert_eq!(frame.chunks().len(), 1);
                        assert!(matches!(&frame.chunks()[0], trace::Chunk::Straightline(_)));
                    }
                    _ => panic!("Expected frame chunk"),
                }
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    #[test]
    fn build_trace_nested() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE1.start, TEST_SYMBOL.clone());
        let mut builder = extract_builder(builder.complete_frame(INNER_RANGE1.end).unwrap());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        let builder = extract_builder(builder.complete_frame(INNER_RANGE2.end).unwrap());
        let builder = extract_builder(builder.complete_frame(SAMPLE_RANGE.end).unwrap());
        match builder.complete_frame(SAMPLE_RANGE.end).unwrap() {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root.chunks().len(), 1);
                match trace.root.chunks()[0] {
                    trace::Chunk::Frame(ref frame) => {
                        assert_eq!(frame.chunks().len(), 4);
                        assert!(matches!(&frame.chunks()[0], trace::Chunk::Straightline(_)));
                        assert!(matches!(&frame.chunks()[2], trace::Chunk::Straightline(_)));

                        match &frame.chunks()[1] {
                            trace::Chunk::Frame(frame) => {
                                assert_eq!(frame.metrics(), &INNER_RANGE1);
                                assert_eq!(frame.chunks().len(), 1);
                                assert!(matches!(
                                    &frame.chunks()[0],
                                    trace::Chunk::Straightline(_)
                                ));
                            }
                            _ => panic!("Expected frame chunk in position 1"),
                        }

                        match &frame.chunks()[3] {
                            trace::Chunk::Frame(frame) => {
                                assert_eq!(
                                    frame.metrics(),
                                    &MetricsRange::new(INNER_RANGE2.start, SAMPLE_RANGE.end)
                                );
                                assert_eq!(frame.chunks().len(), 2);
                                assert!(matches!(
                                    &frame.chunks()[1],
                                    trace::Chunk::Straightline(_)
                                ));

                                match &frame.chunks()[0] {
                                    trace::Chunk::Frame(inner_frame) => {
                                        assert_eq!(inner_frame.metrics(), &INNER_RANGE2);
                                        assert_eq!(inner_frame.chunks().len(), 1);
                                        assert!(matches!(
                                            &inner_frame.chunks()[0],
                                            trace::Chunk::Straightline(_)
                                        ));
                                    }
                                    _ => panic!("Expected frame chunk in nested position 0"),
                                }
                            }
                            _ => panic!("Expected frame chunk in position 3"),
                        }
                    }
                    _ => panic!("Expected frame chunk"),
                }
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    #[test]
    fn build_trace_pauses() {
        let builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        let paused = builder.pause(SAMPLE_RANGE.start + METRICS_ONE).unwrap();
        let mut resumed = paused.resume(INNER_RANGE1.start);

        resumed.push_frame(INNER_RANGE1.start + METRICS_ONE, TEST_SYMBOL.clone());
        let paused = resumed.pause(INNER_RANGE1.end).unwrap();

        let resumed = paused.resume(INNER_RANGE2.start);
        let builder = extract_builder(
            resumed
                .complete_frame(INNER_RANGE2.end - METRICS_ONE)
                .unwrap(),
        );

        match builder.complete_frame(INNER_RANGE2.end).unwrap() {
            BuilderResult::Builder(_) => panic!("Expected completed trace"),
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root_frame().chunks().len(), 5);
                assert_eq!(
                    trace.root_frame().metrics(),
                    &MetricsRange::new(SAMPLE_RANGE.start, INNER_RANGE2.end)
                );

                let frame1 = extract_frame_chunk(&trace.root_frame().chunks()[0]);
                assert_eq!(frame1.chunks().len(), 1);
                assert_eq!(
                    frame1.metrics(),
                    &MetricsRange::new(SAMPLE_RANGE.start, SAMPLE_RANGE.start + METRICS_ONE)
                );

                let frame2 = extract_frame_chunk(&trace.root_frame().chunks()[2]);
                assert_eq!(frame2.chunks().len(), 2);
                assert_eq!(
                    frame2.metrics(),
                    &MetricsRange::new(INNER_RANGE1.start, INNER_RANGE1.end)
                );

                let nested_frame1 = extract_frame_chunk(&frame2.chunks()[1]);
                assert_eq!(nested_frame1.chunks().len(), 1);
                assert_eq!(
                    nested_frame1.metrics(),
                    &MetricsRange::new(INNER_RANGE1.start + METRICS_ONE, INNER_RANGE1.end)
                );

                let frame3 = extract_frame_chunk(&trace.root_frame().chunks()[4]);
                assert_eq!(frame3.chunks().len(), 2);
                assert_eq!(
                    frame3.metrics(),
                    &MetricsRange::new(INNER_RANGE2.start, INNER_RANGE2.end)
                );

                let nested_frame2 = extract_frame_chunk(&frame3.chunks()[0]);
                assert_eq!(nested_frame2.chunks().len(), 1);
                assert_eq!(
                    nested_frame2.metrics(),
                    &MetricsRange::new(INNER_RANGE2.start, INNER_RANGE2.end - METRICS_ONE)
                );
            }
        }
    }

    #[test]
    fn add_events() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.new_event(10, "Event 1".to_string(), "Description 1".to_string());
        builder.new_event(20, "Event 2".to_string(), "Description 2".to_string());

        builder.event_occured(10, INNER_RANGE2.start);
        builder.event_occured(20, INNER_RANGE1.end);
        builder.event_occured(10, INNER_RANGE1.start);

        let result = builder.complete_frame(SAMPLE_RANGE.end).unwrap();
        match result {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.events.len(), 2);
                if trace.events[0].id == 10 && trace.events[1].id == 20 {
                    assert_eq!(trace.events[0].occurences().len(), 2);
                    assert_eq!(trace.events[1].occurences().len(), 1);
                } else if trace.events[0].id == 20 && trace.events[1].id == 10 {
                    assert_eq!(trace.events[0].occurences().len(), 1);
                    assert_eq!(trace.events[1].occurences().len(), 2);
                } else {
                    panic!("Unexpected event IDs");
                }
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    #[test]
    fn frame_symbol_order() {
        let mut builder = TraceBuilder::new(
            SAMPLE_RANGE.start,
            SymbolInfo {
                name: "top level".to_string(),
                offset: 0,
                size: 0,
            },
        );
        builder.push_frame(
            INNER_RANGE1.start,
            SymbolInfo {
                name: "2nd level".to_string(),
                offset: 0,
                size: 0,
            },
        );
        builder.push_frame(
            INNER_RANGE1.start + METRICS_ONE,
            SymbolInfo {
                name: "3rd level".to_string(),
                offset: 0,
                size: 0,
            },
        );
        assert_eq!(builder.get_frame_symbol(0).name, "3rd level");
        assert_eq!(builder.get_frame_symbol(1).name, "2nd level");
        assert_eq!(builder.get_frame_symbol(2).name, "top level");
    }

    #[test]
    #[should_panic]
    fn non_monotonic_fails() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(SAMPLE_RANGE.start - METRICS_ONE, TEST_SYMBOL.clone());
    }

    #[test]
    #[should_panic]
    fn non_monotonic_fails3() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        assert!(
            builder
                .complete_frame(INNER_RANGE2.start - METRICS_ONE)
                .is_ok()
        );
    }
}
