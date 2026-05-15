use clap::{Parser, ValueEnum};
use pt_fuser::{
    analysis::{
        FrameFinder,
        filter::{self, Filter},
        histogram::HistogramApp,
    },
    trace::{Chunk, Event, Frame, NamedFrame, Trace, TraceError},
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use regex::Regex;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum Action {
    Error,
    Latency,
}

#[derive(Parser)]
struct Cli {
    #[clap(value_enum, help = "The data to visualize")]
    action: Action,
    #[clap(
        long,
        default_value_t = false,
        help = "Whether the input trace files are gzipped"
    )]
    gzip: bool,
    #[clap(
        long,
        help = "A regular expression for filter frames symbols. If not provided, will analyze root frames."
    )]
    name_regex: Option<String>,
    #[clap(long, help = Filter::HELP)]
    filter: Vec<Filter>,
    #[clap(help = "The pt-fuser trace files to analyze")]
    input_files: Vec<String>,
}

fn add_histogram_datapoint<'a, F: Frame + 'a>(
    frames: impl IntoIterator<Item = &'a F>,
    error_event: Option<&Event>,
    data: &mut Vec<f64>,
    action: &Action,
) {
    match action {
        Action::Error => {
            let Some(errors) = error_event else {
                data.push(0f64);
                return;
            };
            let errors = errors.occurences();
            let mut error_index = 0;
            let mut num_errors = 0;
            for frame in frames {
                while error_index < errors.len() && errors[error_index] < frame.metrics().end {
                    if errors[error_index] >= frame.metrics().start {
                        num_errors += 1;
                    }
                    error_index += 1;
                }
            }
            data.push(num_errors as f64);
        }
        Action::Latency => {
            for frame in frames {
                data.push(frame.metrics().total_time() as f64);
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();

    let mut traces = cli
        .input_files
        .par_iter()
        .map(|input| {
            let trace_data = std::fs::read(input).expect("Failed to read pt-fuser trace file");
            Trace::bin_deserialize(&trace_data, cli.gzip).expect("pt-fuser trace file is malformed")
        })
        .collect::<Vec<Trace>>();

    for filter in &cli.filter {
        traces = filter::filter_traces(traces, filter);
    }

    let regex = if let Some(name) = cli.name_regex {
        Some(Regex::new(&name).expect("Invalid regular expression"))
    } else {
        None
    };

    let mut data = Vec::new();
    for trace in &traces {
        let error_event = trace.get_event(TraceError::DataCollectionError as u32);
        if let Some(regex) = &regex {
            let pred = |f: &NamedFrame| regex.is_match(&f.symbol.name);
            let mut frame_finders = Vec::new();
            for chunk in trace.root_frame().chunks() {
                if let Chunk::Frame(frame) = chunk {
                    let frame_finder = FrameFinder::new(frame, &pred);
                    frame_finders.push(frame_finder);
                }
            }
            add_histogram_datapoint(
                frame_finders.into_iter().flatten(),
                error_event,
                &mut data,
                &cli.action,
            );
        } else {
            add_histogram_datapoint(
                std::iter::once(trace.root_frame()),
                error_event,
                &mut data,
                &cli.action,
            );
        }
    }

    let options = eframe::NativeOptions::default();
    let app = match cli.action {
        Action::Error => HistogramApp::new(
            format!("Error Count Distribution of {} traces", traces.len()),
            &data,
            "Error Count".into(),
            "Count".into(),
        ),
        Action::Latency => HistogramApp::new(
            format!("Latency Distribution of {} traces", traces.len()),
            &data,
            "Latency (ns)".into(),
            "Count".into(),
        ),
    };
    eframe::run_native(
        "pt-fuser Metric Analysis",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
}
