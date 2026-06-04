use std::fmt::Display;

pub trait Histogram {
    type Error;

    fn new(name: String, data: &[f64], x_axis: String) -> Self;
    fn show(self) -> Result<(), Self::Error>;
}

struct Bins {
    start: u64,
    step: u64,
    counts: Vec<u64>,
}

impl Bins {
    fn add_datapoint(&mut self, value: f64) {
        if value >= self.start as f64 {
            let index = ((value - self.start as f64) / self.step as f64).floor() as usize;
            if index < self.counts.len() {
                self.counts[index] += 1;
            }
        }
    }

    /// Uses Rice's rule to determine bin width.
    fn from_data(data: &[f64]) -> Self {
        if data.is_empty() {
            return Bins {
                start: 0,
                step: 0,
                counts: Vec::new(),
            };
        }

        let n = data.len() as f64;
        let min = data.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = data.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let range = max - min;
        let bin_width = f64::max(range / (2.0 * n.cbrt()), 1f64);
        let step = bin_width.ceil() as u64;
        let start = min.floor() as u64;
        let mut bins = Bins {
            start,
            step,
            counts: vec![0; ((max.floor() as u64 - start) / step + 1) as usize],
        };

        for datapoint in data {
            bins.add_datapoint(*datapoint);
        }
        bins
    }
}

impl Iterator for Bins {
    type Item = (f64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        if self.counts.is_empty() {
            return None;
        }
        let count = self.counts.remove(0);
        let center = self.start as f64 + self.step as f64 / 2.0;
        self.start += self.step;
        Some((center, count))
    }
}

struct Quartiles {
    min: f64,
    q1: f64,
    median: f64,
    q3: f64,
    max: f64,
}

impl Quartiles {
    fn from_data(data: &[f64]) -> Self {
        if data.is_empty() {
            return Quartiles {
                min: 0.0,
                q1: 0.0,
                median: 0.0,
                q3: 0.0,
                max: 0.0,
            };
        }

        let mut sorted = data.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = *sorted.first().unwrap();
        let max = *sorted.last().unwrap();
        let (q1, median, q3) = if sorted.len() % 2 == 0 {
            (
                sorted[sorted.len() / 4],
                sorted[sorted.len() / 2],
                sorted[3 * sorted.len() / 4],
            )
        } else {
            (
                (sorted[sorted.len() / 4] + sorted[sorted.len() / 4 + 1]) / 2.0,
                (sorted[sorted.len() / 2] + sorted[sorted.len() / 2 + 1]) / 2.0,
                (sorted[3 * sorted.len() / 4] + sorted[3 * sorted.len() / 4 + 1]) / 2.0,
            )
        };
        Quartiles {
            min,
            q1,
            median,
            q3,
            max,
        }
    }
}

impl Display for Quartiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Min: {:.1}, Q1: {:.1}, Median: {:.1}, Q3: {:.1}, Max: {:.1}",
            self.min, self.q1, self.median, self.q3, self.max
        )
    }
}

pub struct HistogramASCII {
    bins: Bins,
    quartiles: Quartiles,
    name: String,
}

impl Histogram for HistogramASCII {
    type Error = ();

    fn new(name: String, data: &[f64], _: String) -> Self {
        Self {
            bins: Bins::from_data(data),
            quartiles: Quartiles::from_data(data),
            name,
        }
    }

    fn show(self) -> Result<(), Self::Error> {
        const BAR_WIDTH: u64 = 50;

        let max_count = self.bins.counts.iter().max().cloned().unwrap_or(0);
        let value_width = max_count.to_string().len();
        let bucket_width = (self.bins.start + self.bins.step * self.bins.counts.len() as u64)
            .to_string()
            .len();

        println!("---  {}  ---", self.name);
        println!("{}\n", self.quartiles);
        let mut start = self.bins.start;
        for count in self.bins.counts {
            let end = start + self.bins.step;
            let bar = "*".repeat(((BAR_WIDTH * count) / max_count) as usize);
            print!(
                "[{start:<bucket_width$}, {end:<bucket_width$}] | {count:<value_width$} : {bar}"
            );
            println!();

            start = end;
        }

        Ok(())
    }
}

#[cfg(feature = "gui")]
mod gui {
    use egui_plot::{Bar, BarChart, Plot};

    use super::{Bins, Histogram, Quartiles};

    pub struct HistogramGUI {
        bars: Vec<Bar>,
        quartiles: Quartiles,
        name: String,
        x_axis: String,
    }

    impl Histogram for HistogramGUI {
        type Error = eframe::Error;

        fn new(name: String, data: &[f64], x_axis: String) -> Self {
            let bins = Bins::from_data(data);

            let mut bars = Vec::new();
            let width = bins.step as f64;
            for (center, count) in bins {
                bars.push(Bar::new(center, count as f64).width(width));
            }

            Self {
                bars,
                quartiles: Quartiles::from_data(data),
                name,
                x_axis,
            }
        }

        fn show(self) -> Result<(), Self::Error> {
            let options = eframe::NativeOptions::default();
            eframe::run_native(
                "pt-fuser Metric Analysis",
                options,
                Box::new(|_cc| Ok(Box::new(self))),
            )
        }
    }

    impl eframe::App for HistogramGUI {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().inner_margin(30.0))
                .show_inside(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading(&self.name);
                        ui.label(self.quartiles.to_string());
                    });
                    Plot::new(&self.name)
                        .x_axis_label(&self.x_axis)
                        .y_axis_label("Count")
                        .set_margin_fraction(egui::vec2(0.05, 0.05))
                        .show(ui, |plot_ui| {
                            let barchart = BarChart::new(&self.name, self.bars.clone())
                                .element_formatter(Box::new(|bar, _chart| {
                                    let width = bar.bar_width;
                                    format!(
                                        "Bucket: [{}, {})\nFrequency: {}",
                                        bar.argument - width / 2.0,
                                        bar.argument + width / 2.0,
                                        bar.value
                                    )
                                }))
                                .color(egui::Color32::LIGHT_BLUE);
                            plot_ui.bar_chart(barchart);
                        });
                });
        }
    }
}

#[cfg(feature = "gui")]
pub use gui::HistogramGUI;
