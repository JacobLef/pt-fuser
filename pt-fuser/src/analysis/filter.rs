use std::{iter, str::FromStr};

use regex::Regex;

use crate::{
    analysis::FrameFinder,
    trace::{Chunk, Event, Frame, NamedFrame, Trace, TraceError},
};

#[derive(Debug, Clone)]
pub struct Filter {
    pub target: Option<Regex>,
    pub errors_min: Option<u32>,
    pub errors_max: Option<u32>,
    pub duration_min: Option<u64>,
    pub duration_max: Option<u64>,
}

impl Filter {
    pub const HELP: &'static str = "A filter in the form \"[target=regex,] [errors_min=num,] [errors_max=num,] \
                                    [duration_min=num,] [duration_max=num]\". Traces with frames violating a filter \
                                    are ignored. If target regex is not provided, filter applies to root frames.";
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            target: None,
            errors_min: None,
            errors_max: None,
            duration_min: None,
            duration_max: None,
        }
    }
}

impl FromStr for Filter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut filter = Filter::default();
        for part in s.split(',') {
            let part = part.trim();
            let mut kv = part.splitn(2, '=');
            let Some(key) = kv.next() else {
                return Err(format!("Invalid filter part: {}", part));
            };
            let Some(value) = kv.next() else {
                return Err(format!("Invalid filter part: {}", part));
            };
            match key {
                "target" => {
                    filter.target =
                        Some(Regex::new(value).map_err(|e| format!("Invalid regex: {}", e))?);
                }
                "errors_min" => {
                    filter.errors_min = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for errors_min: {}", e))?,
                    );
                }
                "errors_max" => {
                    filter.errors_max = Some(
                        value
                            .parse::<u32>()
                            .map_err(|e| format!("Invalid number for errors_max: {}", e))?,
                    );
                }
                "duration_min" => {
                    filter.duration_min = Some(
                        value
                            .parse::<u64>()
                            .map_err(|e| format!("Invalid number for duration_min: {}", e))?,
                    );
                }
                "duration_max" => {
                    filter.duration_max = Some(
                        value
                            .parse::<u64>()
                            .map_err(|e| format!("Invalid number for duration_max: {}", e))?,
                    );
                }
                _ => {
                    return Err(format!("Unknown filter keyword: {}", key));
                }
            }
        }
        Ok(filter)
    }
}

fn run_filter<'a, F: Frame + 'a>(
    frames: impl IntoIterator<Item = &'a F>,
    filter: &Filter,
    errors: Option<&Event>,
) -> bool {
    let mut num_errors = 0;
    let mut error_index = 0;
    for frame in frames {
        if let Some(duration_min) = filter.duration_min {
            if frame.metrics().total_time() < duration_min {
                return false;
            }
        }
        if let Some(duration_max) = filter.duration_max {
            if frame.metrics().total_time() > duration_max {
                return false;
            }
        }

        if let Some(error_events) = errors {
            let error_events = error_events.occurences();
            while error_index < error_events.len()
                && error_events[error_index] < frame.metrics().end
            {
                if error_events[error_index] >= frame.metrics().start {
                    num_errors += 1;
                }
                error_index += 1;
            }
        }
    }

    if let Some(errors_min) = filter.errors_min {
        if num_errors < errors_min {
            return false;
        }
    }
    if let Some(errors_max) = filter.errors_max {
        if num_errors > errors_max {
            return false;
        }
    }

    true
}

pub fn filter_traces(mut traces: Vec<Trace>, filter: &Filter) -> Vec<Trace> {
    traces.retain(|trace| {
        let error_events = trace.get_event(TraceError::DataCollectionError as u32);
        if let Some(target) = &filter.target {
            let pred = |frame: &NamedFrame| target.is_match(&frame.symbol.name);
            let mut frame_finders = Vec::new();
            for chunk in trace.root_frame().chunks() {
                if let Chunk::Frame(frame) = chunk {
                    let frame_finder = FrameFinder::new(frame, &pred);
                    frame_finders.push(frame_finder);
                }
            }

            run_filter(frame_finders.into_iter().flatten(), filter, error_events)
        } else {
            run_filter(iter::once(trace.root_frame()), filter, error_events)
        }
    });
    traces
}
