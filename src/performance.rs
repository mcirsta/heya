//! Opt-in, numeric-only synthetic measurements. Normal account archives never enable it.
use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
pub const PHASES: usize = 8;
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Metric {
    Queue,
    ArchiveWork,
    ViewBuild,
    ViewToSnapshot,
    UiApply,
    IngestToUi,
    IngestToRender,
    CommandToRender,
    Ingest,
    TimelineBuild,
    TranscriptBuild,
    TimelineReuse,
    TimelineAppend,
}
const LABELS: [&str; 13] = [
    "archive queue/lock",
    "archive job",
    "SQLCipher view/projection",
    "view through snapshot publication",
    "UI apply",
    "ingest start to UI apply",
    "ingest start to render complete",
    "command enqueue to render complete",
    "ingest commit",
    "timeline projection build",
    "transcript formatting",
    "timeline cache reuse",
    "timeline append projection",
];
pub struct Probe {
    phase: AtomicUsize,
    samples: Mutex<Vec<Vec<Vec<f64>>>>,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            phase: AtomicUsize::new(0),
            samples: Mutex::new(vec![vec![Vec::new(); LABELS.len()]; PHASES]),
        }
    }
}
impl Probe {
    pub fn phase(&self) -> usize {
        self.phase.load(Ordering::Relaxed)
    }
    pub fn set_phase(&self, phase: usize) {
        self.phase.store(phase, Ordering::Relaxed);
    }
    pub fn record(&self, metric: Metric, duration: Duration) {
        self.record_in(self.phase(), metric, duration);
    }
    pub fn record_in(&self, phase: usize, metric: Metric, duration: Duration) {
        let mut samples = self.samples.lock().unwrap();
        if let Some(values) = samples.get_mut(phase).map(|p| &mut p[metric as usize])
            && values.len() < 8192
        {
            values.push(duration.as_secs_f64() * 1000.);
        }
    }
    pub fn report(&self, phase: usize, name: &str) {
        let mut all = self.samples.lock().unwrap();
        for (label, values) in LABELS.into_iter().zip(&mut all[phase]) {
            if values.is_empty() {
                continue;
            }
            values.sort_by(f64::total_cmp);
            let p = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
            println!(
                "Archive benchmark {name} / {label}: n={}, p50={:.2}ms p95={:.2}ms max={:.2}ms",
                values.len(),
                p(0.5),
                p(0.95),
                values.last().unwrap()
            );
        }
    }
}
