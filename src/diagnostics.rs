//! Always-on reports of unusually slow work, for diagnosing stalls in normal use.
//! Lines carry fixed labels, source locations, durations and counts only; never
//! account identifiers, message content or other private data.
use std::{
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// Background work this long visibly delays a response such as opening a chat.
pub(crate) const SLOW: Duration = Duration::from_secs(1);
/// Event-loop work this long visibly freezes the window.
pub(crate) const SLOW_UI: Duration = Duration::from_millis(250);

pub(crate) fn report(line: std::fmt::Arguments<'_>) {
    let line = format!("Heya slow: {line}");
    #[cfg(test)]
    REPORTS.lock().unwrap().push(line.clone());
    eprintln!("{line}");
}

#[cfg(test)]
static REPORTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Tests run in parallel; callers filter for lines naming their own source.
#[cfg(test)]
pub(crate) fn reports() -> Vec<String> {
    REPORTS.lock().unwrap().clone()
}

type Step = Mutex<Option<(&'static str, Instant)>>;
// The Heya event-loop step in progress, read by the watchdog during a freeze.
static UI_STEP: Step = Mutex::new(None);

/// Names event-loop work until dropped; nested steps restore the outer one.
#[must_use]
pub(crate) struct UiStep(&'static Step, Option<(&'static str, Instant)>);

pub(crate) fn ui_step(label: &'static str) -> UiStep {
    mark(&UI_STEP, label)
}

fn mark(slot: &'static Step, label: &'static str) -> UiStep {
    let mut step = slot.lock().unwrap_or_else(PoisonError::into_inner);
    UiStep(slot, step.replace((label, Instant::now())))
}

impl Drop for UiStep {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = self.1.take();
    }
}

fn ping() -> Option<tokio::sync::oneshot::Receiver<()>> {
    let (answer, answered) = tokio::sync::oneshot::channel();
    slint::invoke_from_event_loop(move || {
        let _ = answer.send(());
    })
    .ok()?;
    Some(answered)
}

/// Pings the event loop each second and reports a ping it does not answer
/// within `SLOW`, naming the Heya step it is running. Abort it when the loop exits.
pub(crate) async fn watch_ui() {
    watch(ping, &UI_STEP).await;
}

async fn watch(ping: impl Fn() -> Option<tokio::sync::oneshot::Receiver<()>>, slot: &Step) {
    // Measure only once the loop runs; startup is traced separately.
    let Some(answered) = ping() else { return };
    if answered.await.is_err() {
        return;
    }
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let posted = Instant::now();
        let Some(mut answered) = ping() else { return };
        if tokio::time::timeout(SLOW, &mut answered).await.is_ok() {
            continue;
        }
        let step = *slot.lock().unwrap_or_else(PoisonError::into_inner);
        match step {
            Some((label, started)) => report(format_args!(
                "UI event loop blocked for {} ms, in {label} for {} ms",
                posted.elapsed().as_millis(),
                started.elapsed().as_millis()
            )),
            None => report(format_args!(
                "UI event loop blocked for {} ms, outside Heya's marked steps (Slint layout, rendering or input)",
                posted.elapsed().as_millis()
            )),
        }
        if answered.await.is_err() {
            return;
        }
        report(format_args!(
            "UI event loop resumed after {} ms",
            posted.elapsed().as_millis()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn eventually(condition: impl Fn(&[String]) -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !condition(&reports()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("expected watchdog report");
    }

    #[tokio::test]
    async fn the_watchdog_names_the_step_that_blocks_the_event_loop() {
        static SLOT: Step = Mutex::new(None);
        let (pings, mut loop_queue) = tokio::sync::mpsc::unbounded_channel();
        let watchdog = tokio::spawn(watch(
            move || {
                let (answer, answered) = tokio::sync::oneshot::channel();
                pings.send(answer).ok()?;
                Some(answered)
            },
            &SLOT,
        ));
        // A responsive loop answers the start-up ping and a regular one.
        for _ in 0..2 {
            loop_queue.recv().await.unwrap().send(()).unwrap();
        }
        let outer = mark(&SLOT, "synthetic outer step");
        let inner = mark(&SLOT, "synthetic watchdog step");
        let stalled = loop_queue.recv().await.unwrap();
        eventually(|lines| {
            lines.iter().any(|line| {
                line.contains("UI event loop blocked for")
                    && line.contains("in synthetic watchdog step for")
            })
        })
        .await;
        drop(inner);
        assert_eq!(
            SLOT.lock().unwrap().map(|s| s.0),
            Some("synthetic outer step")
        );
        drop(outer);
        assert!(SLOT.lock().unwrap().is_none());
        assert!(
            !reports()
                .iter()
                .any(|l| l.contains("UI event loop resumed"))
        );
        stalled.send(()).unwrap();
        eventually(|lines| {
            lines
                .iter()
                .any(|l| l.contains("UI event loop resumed after"))
        })
        .await;
        // Later answered pings stay quiet.
        loop_queue.recv().await.unwrap().send(()).unwrap();
        loop_queue.recv().await.unwrap().send(()).unwrap();
        let blocked = reports()
            .iter()
            .filter(|l| l.contains("UI event loop blocked for"))
            .count();
        assert_eq!(blocked, 1);
        watchdog.abort();
    }
}
