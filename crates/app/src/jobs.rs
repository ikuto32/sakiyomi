//! Background jobs (conversions, mining) with progress, cancellation and a final result.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: u32,
    pub kind: String,
    pub title: String,
    /// 0.0–1.0 when known.
    pub progress: Option<f64>,
    pub message: String,
    pub done: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

pub type JobSink = Arc<dyn Fn(&JobView) + Send + Sync>;

struct Entry {
    cancel: Arc<AtomicBool>,
    view: JobView,
}

pub struct Jobs {
    next: AtomicU32,
    jobs: Arc<Mutex<BTreeMap<u32, Entry>>>,
    sink: JobSink,
}

/// Handle given to a running job.
pub struct JobCtx {
    pub id: u32,
    pub cancel: Arc<AtomicBool>,
    jobs: Arc<Mutex<BTreeMap<u32, Entry>>>,
    sink: JobSink,
    last: Mutex<Instant>,
}

impl JobCtx {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Report progress; events are throttled to about ten per second.
    pub fn progress(&self, fraction: Option<f64>, message: impl Into<String>) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < Duration::from_millis(100) {
            return;
        }
        *last = Instant::now();
        let view = {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            let Some(e) = jobs.get_mut(&self.id) else { return };
            e.view.progress = fraction.map(|f| f.clamp(0.0, 1.0));
            e.view.message = message.into();
            e.view.clone()
        };
        (self.sink)(&view);
    }
}

impl Jobs {
    pub fn new(sink: JobSink) -> Self {
        Jobs { next: AtomicU32::new(1), jobs: Arc::new(Mutex::new(BTreeMap::new())), sink }
    }

    pub fn spawn<F>(&self, kind: &str, title: String, f: F) -> u32
    where
        F: FnOnce(&JobCtx) -> Result<serde_json::Value, String> + Send + 'static,
    {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        let view = JobView { id, kind: kind.to_owned(), title, ..Default::default() };
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Entry { cancel: cancel.clone(), view: view.clone() });
        (self.sink)(&view);
        let ctx = JobCtx { id, cancel, jobs: self.jobs.clone(), sink: self.sink.clone(), last: Mutex::new(Instant::now() - Duration::from_secs(1)) };
        std::thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || {
                // A panic in a job must not take the application down.
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&ctx)))
                    .unwrap_or_else(|_| Err("internal error".to_owned()));
                let view = {
                    let mut jobs = ctx.jobs.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(e) = jobs.get_mut(&id) else { return };
                    e.view.done = true;
                    e.view.cancelled = ctx.cancelled();
                    match r {
                        Ok(v) => {
                            e.view.progress = Some(1.0);
                            e.view.result = Some(v);
                        }
                        Err(err) => e.view.error = Some(err),
                    }
                    e.view.clone()
                };
                (ctx.sink)(&view);
            })
            .ok();
        id
    }

    pub fn cancel(&self, id: u32) {
        if let Some(e) = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
            e.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn cancel_all(&self) {
        for e in self.jobs.lock().unwrap_or_else(|e| e.into_inner()).values() {
            e.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn list(&self) -> Vec<JobView> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).values().map(|e| e.view.clone()).collect()
    }

    /// Wait until every job has finished, at most `timeout`; true when all are done.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let t0 = Instant::now();
        loop {
            if self.jobs.lock().unwrap_or_else(|e| e.into_inner()).values().all(|e| e.view.done) {
                return true;
            }
            if t0.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Forget finished jobs.
    pub fn clear_finished(&self) {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, e| !e.view.done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_cancel_and_panic() {
        let seen = Arc::new(Mutex::new(Vec::<JobView>::new()));
        let s2 = seen.clone();
        let jobs = Jobs::new(Arc::new(move |v: &JobView| s2.lock().unwrap().push(v.clone())));
        let ok = jobs.spawn("t", "ok".into(), |c| {
            c.progress(Some(0.5), "half");
            Ok(serde_json::json!(42))
        });
        let slow = jobs.spawn("t", "slow".into(), |c| {
            while !c.cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err("stopped".into())
        });
        let boom = jobs.spawn("t", "boom".into(), |_| panic!("boom"));
        jobs.cancel(slow);
        let t0 = Instant::now();
        while jobs.list().iter().any(|j| !j.done) {
            assert!(t0.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let list = jobs.list();
        let get = |id| list.iter().find(|j| j.id == id).unwrap().clone();
        assert_eq!(get(ok).result, Some(serde_json::json!(42)));
        assert!(get(slow).cancelled);
        assert_eq!(get(boom).error.as_deref(), Some("internal error"));
        jobs.clear_finished();
        assert!(jobs.list().is_empty());
    }
}
