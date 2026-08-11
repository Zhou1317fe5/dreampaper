use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct JobControl {
    cancelled: Arc<AtomicBool>,
    abort: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
pub struct CancelRegistry {
    entries: Mutex<HashMap<String, JobControl>>,
}

impl CancelRegistry {
    pub fn register(
        &self,
        job_id: &str,
        cancelled: Arc<AtomicBool>,
        abort: impl Fn() + Send + Sync + 'static,
    ) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                job_id.to_string(),
                JobControl {
                    cancelled,
                    abort: Box::new(abort),
                },
            );
        }
    }

    pub fn finish(&self, job_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(job_id);
        }
    }

    pub fn cancel(&self, job_id: &str) -> bool {
        let control = match self.entries.lock() {
            Ok(mut entries) => entries.remove(job_id),
            Err(_) => None,
        };
        match control {
            Some(control) => {
                control.cancelled.store(true, Ordering::SeqCst);
                (control.abort)();
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_sets_the_flag_and_fires_abort_once() {
        let registry = CancelRegistry::default();
        let flag = Arc::new(AtomicBool::new(false));
        let aborted = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&aborted);
        registry.register("job-1", Arc::clone(&flag), move || {
            seen.store(true, Ordering::SeqCst)
        });

        assert!(registry.cancel("job-1"));
        assert!(flag.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
        assert!(!registry.cancel("job-1"));
    }

    #[test]
    fn finished_jobs_are_no_longer_cancellable() {
        let registry = CancelRegistry::default();
        registry.register("job-2", Arc::new(AtomicBool::new(false)), || {});
        registry.finish("job-2");
        assert!(!registry.cancel("job-2"));
    }
}
